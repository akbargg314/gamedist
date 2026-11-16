//! Authenticated REST API: developer and admin endpoints (SPEC §5.2, §5.3).

use crate::{
    auth::{self, AdminUser, AuthUser, ClientIp},
    db,
    error::{ApiError, ApiResult},
    games,
    metrics::Metrics,
    state::St,
};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use gamedist_core::{
    api::*,
    manifest::{GameManifest, Registration},
    ChunkId,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::Row;

// ---- accounts ------------------------------------------------------------

/// Create an account, enforcing the registration mode. Shared with the portal.
pub async fn register_account(
    st: &St,
    email: &str,
    password: &str,
    invite: Option<&str>,
) -> ApiResult<i64> {
    let email = auth::validate_email(email)?;
    auth::validate_password(password)?;
    let mode = db::registration_mode(&st.db, st.cfg.registration).await?;
    let user_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(&st.db)
        .await?;
    match mode {
        Registration::Closed => {
            return Err(ApiError::forbidden(
                "registration is closed on this server; ask the operator for an account",
            ))
        }
        Registration::Invite => {
            let code = invite
                .map(str::trim)
                .filter(|c| !c.is_empty())
                .ok_or_else(|| ApiError::forbidden("this server requires an invite code"))?;
            let valid: Option<i64> =
                sqlx::query_scalar("SELECT 1 FROM invites WHERE code = ? AND used_by IS NULL")
                    .bind(code)
                    .fetch_optional(&st.db)
                    .await?;
            if valid.is_none() {
                return Err(ApiError::forbidden("invalid or already used invite code"));
            }
        }
        Registration::Open => {}
    }
    let hash = auth::hash_password(password.to_string()).await?;
    // On an open server the very first account becomes the administrator.
    let is_admin = user_count == 0 && mode == Registration::Open;
    let user_id = insert_user(st, &email, &hash, is_admin).await?;
    if mode == Registration::Invite {
        let claimed =
            sqlx::query("UPDATE invites SET used_by = ? WHERE code = ? AND used_by IS NULL")
                .bind(user_id)
                .bind(invite.unwrap_or_default().trim())
                .execute(&st.db)
                .await?;
        if claimed.rows_affected() == 0 {
            // Someone used the code between the check and now.
            sqlx::query("DELETE FROM users WHERE id = ?")
                .bind(user_id)
                .execute(&st.db)
                .await?;
            return Err(ApiError::forbidden("invalid or already used invite code"));
        }
    }
    db::audit(&st.db, Some((user_id, &email)), "user.register", &email, "").await;
    Ok(user_id)
}

pub async fn insert_user(st: &St, email: &str, hash: &str, is_admin: bool) -> ApiResult<i64> {
    let result = sqlx::query(
        "INSERT INTO users (email, password_hash, is_admin, created_at) VALUES (?, ?, ?, ?)",
    )
    .bind(email)
    .bind(hash)
    .bind(is_admin)
    .bind(db::now())
    .execute(&st.db)
    .await;
    match result {
        Ok(done) => Ok(done.last_insert_rowid()),
        Err(sqlx::Error::Database(e)) if e.is_unique_violation() => Err(ApiError::conflict(
            "an account with that email already exists",
        )),
        Err(e) => Err(e.into()),
    }
}

/// Check credentials. Shared with the portal.
pub async fn login(st: &St, email: &str, password: &str) -> ApiResult<auth::User> {
    let email = email.trim().to_lowercase();
    let row: Option<(i64, String, i64, i64)> =
        sqlx::query_as("SELECT id, password_hash, is_admin, disabled FROM users WHERE email = ?")
            .bind(&email)
            .fetch_optional(&st.db)
            .await?;
    // Always run the hash so unknown emails are not distinguishable by timing.
    let (id, hash, is_admin, disabled) = row.unwrap_or((
        0,
        "$argon2id$v=19$m=19456,t=2,p=1$c29tZXNhbHRzb21lc2FsdA$5Yc2KyPzYqDgk0vXLQ1a0H4Cq2R3bCqQv0b3n1dX8Vg"
            .to_string(),
        0,
        1,
    ));
    let ok = auth::verify_password(password.to_string(), hash).await;
    if !ok || id == 0 {
        Metrics::add(&st.metrics.auth_failures, 1);
        return Err(ApiError::unauthorized("wrong email or password"));
    }
    if disabled != 0 {
        return Err(ApiError::forbidden("this account has been disabled"));
    }
    Ok(auth::User {
        id,
        email,
        is_admin: is_admin != 0,
    })
}

pub async fn register(
    State(st): State<St>,
    ClientIp(ip): ClientIp,
    Json(req): Json<RegisterRequest>,
) -> ApiResult<Response> {
    st.auth_limiter.check(ip)?;
    register_account(&st, &req.email, &req.password, req.invite.as_deref()).await?;
    Ok((StatusCode::CREATED, Json(json!({"ok": true}))).into_response())
}

pub async fn token(
    State(st): State<St>,
    ClientIp(ip): ClientIp,
    Json(req): Json<TokenRequest>,
) -> ApiResult<Json<TokenResponse>> {
    st.auth_limiter.check(ip)?;
    let user = login(&st, &req.email, &req.password).await?;
    let name = req.name.as_deref().unwrap_or("api token");
    let token = auth::create_token(&st.db, user.id, name, "api").await?;
    db::audit(&st.db, user.actor(), "token.create", name, "").await;
    Ok(Json(TokenResponse { token }))
}

pub async fn me(State(st): State<St>, AuthUser(user): AuthUser) -> ApiResult<Json<Me>> {
    let keys: Vec<(String, String)> =
        sqlx::query_as("SELECT pubkey, label FROM dev_keys WHERE user_id = ? ORDER BY id")
            .bind(user.id)
            .fetch_all(&st.db)
            .await?;
    let games: Vec<String> =
        sqlx::query_scalar("SELECT slug FROM games WHERE owner_id = ? ORDER BY slug")
            .bind(user.id)
            .fetch_all(&st.db)
            .await?;
    Ok(Json(Me {
        keys: keys
            .into_iter()
            .filter_map(|(pubkey, label)| {
                Some(DevKey {
                    pubkey: pubkey.parse().ok()?,
                    label,
                })
            })
            .collect(),
        games,
        storage_used: db::storage_used(&st.db, user.id).await?,
        storage_quota: db::quota_for(&st.db, user.id, st.cfg.default_quota_bytes).await?,
        email: user.email,
        is_admin: user.is_admin,
    }))
}

pub async fn add_key(
    State(st): State<St>,
    AuthUser(user): AuthUser,
    Json(req): Json<AddKeyRequest>,
) -> ApiResult<Response> {
    let created =
        games::register_key(&st, &user, req.pubkey, req.label.as_deref().unwrap_or("")).await?;
    let status = if created {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok((status, Json(json!({"pubkey": req.pubkey}))).into_response())
}

// ---- games ---------------------------------------------------------------

pub async fn create_game(
    State(st): State<St>,
    AuthUser(user): AuthUser,
    Json(req): Json<CreateGameRequest>,
) -> ApiResult<Response> {
    let game = games::create_game(
        &st,
        &user,
        &req.slug,
        req.developer_pubkey,
        &req.title,
        &req.summary,
        &req.description_md,
    )
    .await?;
    let manifest = games::manifest_for(&st, &game).await?;
    Ok((StatusCode::CREATED, Json(manifest)).into_response())
}

pub async fn patch_game(
    State(st): State<St>,
    AuthUser(user): AuthUser,
    Path(slug): Path<String>,
    Json(req): Json<PatchGameRequest>,
) -> ApiResult<Json<GameManifest>> {
    let game = games::update_game(
        &st,
        &user,
        &slug,
        req.title.as_deref(),
        req.summary.as_deref(),
        req.description_md.as_deref(),
        req.assets.as_ref(),
    )
    .await?;
    Ok(Json(games::manifest_for(&st, &game).await?))
}

pub async fn missing_chunks(
    State(st): State<St>,
    AuthUser(_user): AuthUser,
    Json(req): Json<MissingChunksRequest>,
) -> ApiResult<Json<MissingChunksResponse>> {
    if req.ids.len() > 10_000 {
        return Err(ApiError::bad_request("at most 10000 ids per request"));
    }
    let mut missing = Vec::new();
    for batch in req.ids.chunks(500) {
        let placeholders = vec!["?"; batch.len()].join(",");
        let sql = format!("SELECT id FROM chunks WHERE id IN ({placeholders})");
        let mut query = sqlx::query_scalar::<_, String>(&sql);
        for id in batch {
            query = query.bind(id.to_hex());
        }
        let present: std::collections::HashSet<String> =
            query.fetch_all(&st.db).await?.into_iter().collect();
        missing.extend(
            batch
                .iter()
                .filter(|id| !present.contains(&id.to_hex()))
                .copied(),
        );
    }
    Ok(Json(MissingChunksResponse { missing }))
}

pub async fn put_chunk(
    State(st): State<St>,
    AuthUser(user): AuthUser,
    ClientIp(ip): ClientIp,
    Path(id): Path<String>,
    body: bytes::Bytes,
) -> ApiResult<StatusCode> {
    st.upload_limiter.check(ip)?;
    let id: ChunkId = id.parse()?;
    if games::store_chunk(&st, &user, id, body).await? {
        Ok(StatusCode::CREATED)
    } else {
        Ok(StatusCode::NO_CONTENT)
    }
}

pub async fn publish_release(
    State(st): State<St>,
    AuthUser(user): AuthUser,
    Path(slug): Path<String>,
    body: String,
) -> ApiResult<Response> {
    let created = games::publish_release(&st, &user, &slug, &body).await?;
    let status = if created {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok((status, Json(json!({"created": created}))).into_response())
}

pub async fn set_channel(
    State(st): State<St>,
    AuthUser(user): AuthUser,
    Path(slug): Path<String>,
    Json(req): Json<SetChannelRequest>,
) -> ApiResult<Json<Value>> {
    games::set_channel(&st, &user, &slug, &req.channel, &req.version).await?;
    Ok(Json(
        json!({"channel": req.channel, "version": req.version}),
    ))
}

pub async fn post_rotation(
    State(st): State<St>,
    AuthUser(user): AuthUser,
    Path(slug): Path<String>,
    Json(statement): Json<Value>,
) -> ApiResult<Json<Value>> {
    games::rotate_key(&st, &user, &slug, &statement).await?;
    Ok(Json(json!({"ok": true})))
}

pub async fn delete_release(
    State(st): State<St>,
    AuthUser(user): AuthUser,
    Path((slug, version)): Path<(String, String)>,
) -> ApiResult<StatusCode> {
    games::delete_release(&st, &user, &slug, &version).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---- admin ---------------------------------------------------------------

pub async fn admin_users(State(st): State<St>, AdminUser(_): AdminUser) -> ApiResult<Json<Value>> {
    let rows = sqlx::query(
        "SELECT u.id, u.email, u.is_admin, u.disabled, u.quota_bytes, u.created_at,
                (SELECT COALESCE(SUM(size), 0) FROM chunks c WHERE c.uploader_id = u.id) AS used,
                (SELECT COUNT(*) FROM games g WHERE g.owner_id = u.id) AS games
         FROM users u ORDER BY u.id",
    )
    .fetch_all(&st.db)
    .await?;
    let users: Vec<Value> = rows
        .iter()
        .map(|r| {
            json!({
                "id": r.get::<i64, _>("id"),
                "email": r.get::<String, _>("email"),
                "is_admin": r.get::<i64, _>("is_admin") != 0,
                "disabled": r.get::<i64, _>("disabled") != 0,
                "quota_bytes": r.get::<Option<i64>, _>("quota_bytes"),
                "storage_used": r.get::<i64, _>("used"),
                "games": r.get::<i64, _>("games"),
                "created_at": r.get::<i64, _>("created_at"),
            })
        })
        .collect();
    Ok(Json(json!({ "users": users })))
}

#[derive(Debug, Deserialize)]
pub struct UpdateUser {
    pub id: i64,
    #[serde(default)]
    pub disabled: Option<bool>,
    #[serde(default)]
    pub is_admin: Option<bool>,
    /// `null`/absent leaves it alone; `-1` resets to the server default;
    /// `0` is unlimited.
    #[serde(default)]
    pub quota_bytes: Option<i64>,
}

/// Apply an admin change to a user. Shared with the portal.
pub async fn update_user(st: &St, admin: &auth::User, req: &UpdateUser) -> ApiResult<()> {
    let email: String = sqlx::query_scalar("SELECT email FROM users WHERE id = ?")
        .bind(req.id)
        .fetch_optional(&st.db)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("user {}", req.id)))?;
    if req.id == admin.id && (req.disabled == Some(true) || req.is_admin == Some(false)) {
        return Err(ApiError::bad_request(
            "you cannot disable or demote your own account",
        ));
    }
    if let Some(disabled) = req.disabled {
        sqlx::query("UPDATE users SET disabled = ? WHERE id = ?")
            .bind(disabled)
            .bind(req.id)
            .execute(&st.db)
            .await?;
        if disabled {
            sqlx::query("DELETE FROM tokens WHERE user_id = ?")
                .bind(req.id)
                .execute(&st.db)
                .await?;
        }
    }
    if let Some(is_admin) = req.is_admin {
        sqlx::query("UPDATE users SET is_admin = ? WHERE id = ?")
            .bind(is_admin)
            .bind(req.id)
            .execute(&st.db)
            .await?;
    }
    if let Some(quota) = req.quota_bytes {
        sqlx::query("UPDATE users SET quota_bytes = ? WHERE id = ?")
            .bind((quota >= 0).then_some(quota))
            .bind(req.id)
            .execute(&st.db)
            .await?;
    }
    db::audit(
        &st.db,
        admin.actor(),
        "user.update",
        &email,
        &format!(
            "disabled={:?} is_admin={:?} quota_bytes={:?}",
            req.disabled, req.is_admin, req.quota_bytes
        ),
    )
    .await;
    Ok(())
}

pub async fn admin_update_user(
    State(st): State<St>,
    AdminUser(admin): AdminUser,
    Json(req): Json<UpdateUser>,
) -> ApiResult<Json<Value>> {
    update_user(&st, &admin, &req).await?;
    Ok(Json(json!({"ok": true})))
}

pub async fn admin_games(State(st): State<St>, AdminUser(_): AdminUser) -> ApiResult<Json<Value>> {
    let rows = sqlx::query(
        "SELECT g.slug, g.title, g.taken_down, g.takedown_reason, u.email,
                (SELECT COUNT(*) FROM releases r WHERE r.game_id = g.id) AS releases
         FROM games g JOIN users u ON u.id = g.owner_id ORDER BY g.slug",
    )
    .fetch_all(&st.db)
    .await?;
    let games: Vec<Value> = rows
        .iter()
        .map(|r| {
            json!({
                "slug": r.get::<String, _>("slug"),
                "title": r.get::<String, _>("title"),
                "owner": r.get::<String, _>("email"),
                "taken_down": r.get::<i64, _>("taken_down") != 0,
                "takedown_reason": r.get::<String, _>("takedown_reason"),
                "releases": r.get::<i64, _>("releases"),
            })
        })
        .collect();
    Ok(Json(json!({ "games": games })))
}

pub async fn admin_takedowns(
    State(st): State<St>,
    AdminUser(_): AdminUser,
) -> ApiResult<Json<Value>> {
    let rows: Vec<(String, String, String, i64)> =
        sqlx::query_as("SELECT slug, action, reason, at FROM takedowns ORDER BY id DESC LIMIT 500")
            .fetch_all(&st.db)
            .await?;
    let list: Vec<Value> = rows
        .into_iter()
        .map(|(slug, action, reason, at)| json!({"slug": slug, "action": action, "reason": reason, "at": at}))
        .collect();
    Ok(Json(json!({ "takedowns": list })))
}

#[derive(Debug, Deserialize)]
pub struct TakedownRequest {
    pub slug: String,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub restore: bool,
}

pub async fn admin_takedown(
    State(st): State<St>,
    AdminUser(admin): AdminUser,
    Json(req): Json<TakedownRequest>,
) -> ApiResult<Json<Value>> {
    games::takedown(&st, &admin, &req.slug, &req.reason, req.restore).await?;
    Ok(Json(json!({"ok": true})))
}

pub async fn admin_settings(
    State(st): State<St>,
    AdminUser(_): AdminUser,
) -> ApiResult<Json<Value>> {
    Ok(Json(json!({
        "registration": db::registration_mode(&st.db, st.cfg.registration).await?.as_str(),
        "default_quota_bytes": db::default_quota(&st.db, st.cfg.default_quota_bytes).await?,
    })))
}

#[derive(Debug, Deserialize)]
pub struct SettingsRequest {
    #[serde(default)]
    pub registration: Option<String>,
    #[serde(default)]
    pub default_quota_bytes: Option<u64>,
}

/// Shared with the portal.
pub async fn update_settings(st: &St, admin: &auth::User, req: &SettingsRequest) -> ApiResult<()> {
    if let Some(mode) = &req.registration {
        let mode = Registration::parse(mode)?;
        db::set_setting(&st.db, "registration", mode.as_str()).await?;
        db::audit(
            &st.db,
            admin.actor(),
            "settings.registration",
            mode.as_str(),
            "",
        )
        .await;
    }
    if let Some(quota) = req.default_quota_bytes {
        db::set_setting(&st.db, "default_quota_bytes", &quota.to_string()).await?;
        db::audit(
            &st.db,
            admin.actor(),
            "settings.default_quota",
            &quota.to_string(),
            "",
        )
        .await;
    }
    Ok(())
}

pub async fn admin_update_settings(
    State(st): State<St>,
    AdminUser(admin): AdminUser,
    Json(req): Json<SettingsRequest>,
) -> ApiResult<Json<Value>> {
    update_settings(&st, &admin, &req).await?;
    admin_settings(State(st), AdminUser(admin)).await
}

/// Shared with the portal.
pub async fn create_invite(st: &St, admin: &auth::User) -> ApiResult<String> {
    use base64::Engine;
    use rand::RngCore;
    let mut raw = [0u8; 15];
    rand::rngs::OsRng.fill_bytes(&mut raw);
    let code = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw);
    sqlx::query("INSERT INTO invites (code, created_by, created_at) VALUES (?, ?, ?)")
        .bind(&code)
        .bind(admin.id)
        .bind(db::now())
        .execute(&st.db)
        .await?;
    db::audit(&st.db, admin.actor(), "invite.create", "", "").await;
    Ok(code)
}

pub async fn admin_create_invite(
    State(st): State<St>,
    AdminUser(admin): AdminUser,
) -> ApiResult<Response> {
    let code = create_invite(&st, &admin).await?;
    Ok((StatusCode::CREATED, Json(json!({ "code": code }))).into_response())
}

pub async fn admin_audit(State(st): State<St>, AdminUser(_): AdminUser) -> ApiResult<Json<Value>> {
    let rows: Vec<(i64, String, String, String, String)> = sqlx::query_as(
        "SELECT at, actor, action, target, detail FROM audit_log ORDER BY id DESC LIMIT 500",
    )
    .fetch_all(&st.db)
    .await?;
    let entries: Vec<Value> = rows
        .into_iter()
        .map(|(at, actor, action, target, detail)| {
            json!({"at": at, "actor": actor, "action": action, "target": target, "detail": detail})
        })
        .collect();
    Ok(Json(json!({ "entries": entries })))
}
