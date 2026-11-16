//! Accounts, password hashing, API/session tokens, request extractors and
//! per-IP rate limiting.

use crate::{
    db,
    error::{ApiError, ApiResult},
    state::St,
};
use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use axum::{
    extract::{ConnectInfo, FromRequestParts},
    http::{header, request::Parts},
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD as B64, Engine};
use rand::RngCore;
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    sync::Mutex,
    time::{Duration, Instant},
};

pub const SESSION_COOKIE: &str = "gd_session";
const SESSION_TTL_SECS: i64 = 30 * 24 * 3600;

pub async fn hash_password(password: String) -> ApiResult<String> {
    tokio::task::spawn_blocking(move || {
        let salt = SaltString::generate(&mut OsRng);
        Argon2::default()
            .hash_password(password.as_bytes(), &salt)
            .map(|h| h.to_string())
    })
    .await
    .map_err(|e| ApiError::internal("hash task", e))?
    .map_err(|e| ApiError::internal("argon2", e))
}

pub async fn verify_password(password: String, hash: String) -> bool {
    tokio::task::spawn_blocking(move || {
        PasswordHash::new(&hash)
            .map(|parsed| {
                Argon2::default()
                    .verify_password(password.as_bytes(), &parsed)
                    .is_ok()
            })
            .unwrap_or(false)
    })
    .await
    .unwrap_or(false)
}

pub fn validate_email(email: &str) -> ApiResult<String> {
    let email = email.trim().to_lowercase();
    let ok = email.len() <= 254
        && email
            .split_once('@')
            .is_some_and(|(local, domain)| !local.is_empty() && domain.contains('.'))
        && !email.chars().any(|c| c.is_whitespace() || c.is_control());
    if ok {
        Ok(email)
    } else {
        Err(ApiError::bad_request(
            "that does not look like an email address",
        ))
    }
}

pub fn validate_password(password: &str) -> ApiResult<()> {
    if password.chars().count() < 10 {
        return Err(ApiError::bad_request(
            "password must be at least 10 characters",
        ));
    }
    if password.len() > 1024 {
        return Err(ApiError::bad_request("password is too long"));
    }
    Ok(())
}

fn token_hash(token: &str) -> String {
    data_encoding::HEXLOWER.encode(&Sha256::digest(token.as_bytes()))
}

/// Create a token for `user`; returns the secret, which is shown exactly once.
pub async fn create_token(db: &SqlitePool, user: i64, name: &str, kind: &str) -> ApiResult<String> {
    let mut raw = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut raw);
    let token = format!("gd_{}", B64.encode(raw));
    let expires = (kind == "session").then(|| db::now() + SESSION_TTL_SECS);
    sqlx::query(
        "INSERT INTO tokens (user_id, name, hash, kind, created_at, expires_at) VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(user)
    .bind(name.chars().take(100).collect::<String>())
    .bind(token_hash(&token))
    .bind(kind)
    .bind(db::now())
    .bind(expires)
    .execute(db)
    .await?;
    Ok(token)
}

pub async fn revoke_token(db: &SqlitePool, token: &str) -> ApiResult<()> {
    sqlx::query("DELETE FROM tokens WHERE hash = ?")
        .bind(token_hash(token))
        .execute(db)
        .await?;
    Ok(())
}

#[derive(Debug, Clone)]
pub struct User {
    pub id: i64,
    pub email: String,
    pub is_admin: bool,
}

impl User {
    pub fn actor(&self) -> Option<(i64, &str)> {
        Some((self.id, &self.email))
    }
}

async fn user_for_token(db: &SqlitePool, token: &str, kind: &str) -> ApiResult<Option<User>> {
    let row: Option<(i64, i64, String, i64, i64, Option<i64>)> = sqlx::query_as(
        "SELECT t.id, u.id, u.email, u.is_admin, u.disabled, t.expires_at
         FROM tokens t JOIN users u ON u.id = t.user_id
         WHERE t.hash = ? AND t.kind = ?",
    )
    .bind(token_hash(token))
    .bind(kind)
    .fetch_optional(db)
    .await?;
    let Some((token_id, id, email, is_admin, disabled, expires)) = row else {
        return Ok(None);
    };
    if disabled != 0 || expires.is_some_and(|e| e < db::now()) {
        return Ok(None);
    }
    // Best effort; a failed bookkeeping write must not fail the request.
    let _ = sqlx::query("UPDATE tokens SET last_used_at = ? WHERE id = ?")
        .bind(db::now())
        .bind(token_id)
        .execute(db)
        .await;
    Ok(Some(User {
        id,
        email,
        is_admin: is_admin != 0,
    }))
}

/// A developer authenticated with `Authorization: Bearer <token>`.
pub struct AuthUser(pub User);

impl FromRequestParts<St> for AuthUser {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &St) -> Result<Self, ApiError> {
        let token = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .ok_or_else(|| ApiError::unauthorized("missing bearer token"))?;
        user_for_token(&state.db, token.trim(), "api")
            .await?
            .map(AuthUser)
            .ok_or_else(|| ApiError::unauthorized("invalid, expired or revoked token"))
    }
}

/// An authenticated administrator (API).
pub struct AdminUser(pub User);

impl FromRequestParts<St> for AdminUser {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &St) -> Result<Self, ApiError> {
        let AuthUser(user) = AuthUser::from_request_parts(parts, state).await?;
        if !user.is_admin {
            return Err(ApiError::forbidden("administrator access required"));
        }
        Ok(AdminUser(user))
    }
}

pub fn cookie_value<'a>(headers: &'a axum::http::HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v)
}

/// A portal visitor; `user` is set when a valid session cookie is present.
pub struct Session {
    pub user: Option<User>,
    /// Token expected in every state-changing portal form.
    pub csrf: String,
    token: Option<String>,
}

impl Session {
    pub fn token(&self) -> Option<&str> {
        self.token.as_deref()
    }

    pub fn check_csrf(&self, submitted: &str) -> ApiResult<()> {
        if self.user.is_some() && !self.csrf.is_empty() && submitted == self.csrf {
            Ok(())
        } else {
            Err(ApiError::forbidden(
                "form expired or invalid (CSRF check failed); reload the page and retry",
            ))
        }
    }
}

pub fn csrf_for(session_token: &str) -> String {
    data_encoding::HEXLOWER.encode(&Sha256::digest(format!("csrf:{session_token}").as_bytes()))
        [..32]
        .to_string()
}

impl FromRequestParts<St> for Session {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &St) -> Result<Self, ApiError> {
        let token = cookie_value(&parts.headers, SESSION_COOKIE).map(str::to_string);
        let user = match &token {
            Some(token) => user_for_token(&state.db, token, "session").await?,
            None => None,
        };
        let csrf = match (&user, &token) {
            (Some(_), Some(token)) => csrf_for(token),
            _ => String::new(),
        };
        Ok(Session { user, csrf, token })
    }
}

/// The client's IP address, honouring `X-Forwarded-For` when configured.
pub struct ClientIp(pub IpAddr);

impl FromRequestParts<St> for ClientIp {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &St) -> Result<Self, ApiError> {
        if state.cfg.trust_proxy {
            // The right-most entry is the one our own proxy appended.
            if let Some(ip) = parts
                .headers
                .get("x-forwarded-for")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.rsplit(',').next())
                .and_then(|v| v.trim().parse().ok())
            {
                return Ok(ClientIp(ip));
            }
        }
        let ip = parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|c| c.0.ip())
            .unwrap_or(IpAddr::from([127, 0, 0, 1]));
        Ok(ClientIp(ip))
    }
}

/// Fixed-window per-IP rate limiter.
pub struct Limiter {
    per_minute: u32,
    windows: Mutex<HashMap<IpAddr, (Instant, u32)>>,
}

impl Limiter {
    pub fn new(per_minute: u32) -> Self {
        Limiter {
            per_minute,
            windows: Mutex::new(HashMap::new()),
        }
    }

    pub fn check(&self, ip: IpAddr) -> ApiResult<()> {
        self.check_at(ip, Instant::now())
    }

    fn check_at(&self, ip: IpAddr, now: Instant) -> ApiResult<()> {
        const WINDOW: Duration = Duration::from_secs(60);
        let mut windows = self.windows.lock().unwrap_or_else(|e| e.into_inner());
        if windows.len() > 10_000 {
            windows.retain(|_, (start, _)| now.duration_since(*start) < WINDOW);
        }
        let entry = windows.entry(ip).or_insert((now, 0));
        if now.duration_since(entry.0) >= WINDOW {
            *entry = (now, 0);
        }
        entry.1 += 1;
        if entry.1 > self.per_minute {
            Err(ApiError::too_many_requests())
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limiter_windows() {
        let limiter = Limiter::new(2);
        let ip: IpAddr = "10.0.0.1".parse().unwrap();
        let other: IpAddr = "10.0.0.2".parse().unwrap();
        let t0 = Instant::now();
        limiter.check_at(ip, t0).unwrap();
        limiter.check_at(ip, t0).unwrap();
        assert!(limiter.check_at(ip, t0).is_err());
        limiter.check_at(other, t0).unwrap();
        limiter.check_at(ip, t0 + Duration::from_secs(61)).unwrap();
    }

    #[test]
    fn emails_and_passwords() {
        assert_eq!(
            validate_email(" Dev@Example.COM ").unwrap(),
            "dev@example.com"
        );
        for bad in ["", "nope", "@x.y", "a@b", "a b@c.d"] {
            assert!(validate_email(bad).is_err(), "{bad}");
        }
        assert!(validate_password("short").is_err());
        validate_password("long enough pw").unwrap();
    }

    #[test]
    fn cookies() {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(header::COOKIE, "a=1; gd_session=tok; b=2".parse().unwrap());
        assert_eq!(cookie_value(&headers, "gd_session"), Some("tok"));
        assert_eq!(cookie_value(&headers, "missing"), None);
    }

    #[tokio::test]
    async fn password_hash_roundtrip() {
        let hash = hash_password("correct horse".into()).await.unwrap();
        assert!(hash.starts_with("$argon2"));
        assert!(verify_password("correct horse".into(), hash.clone()).await);
        assert!(!verify_password("wrong".into(), hash).await);
        assert!(!verify_password("x".into(), "not a hash".into()).await);
    }
}
