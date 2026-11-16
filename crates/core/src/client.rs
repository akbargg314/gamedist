//! Typed HTTP client for a distributor server.

use crate::{
    api::*,
    manifest::{GameManifest, ServerInfo, SignedRelease},
    ChunkId, Error, PublicKey, Result,
};
use serde::{de::DeserializeOwned, Serialize};
use std::time::Duration;

pub const USER_AGENT: &str = concat!("gamedist/", env!("CARGO_PKG_VERSION"));

/// A `reqwest` client with the timeouts every GameDist component uses.
pub fn http_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(Duration::from_secs(15))
        .read_timeout(Duration::from_secs(60))
        .build()
        .map_err(net)
}

pub(crate) fn net(e: reqwest::Error) -> Error {
    // `without_url` keeps query strings (and anything else in URLs) out of logs.
    Error::Network(e.without_url().to_string())
}

/// Turn a non-2xx response into [`Error::Api`], reading the problem document.
pub async fn check(resp: reqwest::Response) -> Result<reqwest::Response> {
    let status = resp.status();
    if status.is_success() {
        return Ok(resp);
    }
    let body = resp.text().await.unwrap_or_default();
    let (title, detail) = match serde_json::from_str::<Problem>(&body) {
        Ok(p) => (p.title, p.detail),
        Err(_) => (
            status.canonical_reason().unwrap_or("error").to_string(),
            body.chars().take(200).collect(),
        ),
    };
    Err(Error::Api {
        status: status.as_u16(),
        title,
        detail,
    })
}

#[derive(Debug, Clone)]
pub struct ApiClient {
    http: reqwest::Client,
    base: String,
    info: ServerInfo,
    token: Option<String>,
}

impl ApiClient {
    /// Fetch `/.well-known/gamedist.json` from `base` and build a client.
    pub async fn connect(base: &str) -> Result<Self> {
        Self::connect_with(http_client()?, base).await
    }

    pub async fn connect_with(http: reqwest::Client, base: &str) -> Result<Self> {
        let base = base.trim_end_matches('/').to_string();
        if !(base.starts_with("https://") || base.starts_with("http://")) {
            return Err(Error::invalid(
                "server url",
                format!("{base:?} must start with https:// or http://"),
            ));
        }
        let resp = http
            .get(format!("{base}/.well-known/gamedist.json"))
            .send()
            .await
            .map_err(net)?;
        let text = check(resp).await?.text().await.map_err(net)?;
        let info = ServerInfo::parse(&text)?;
        if info.protocol != crate::PROTOCOL {
            return Err(Error::invalid(
                "server info",
                format!("unsupported protocol {}", info.protocol),
            ));
        }
        Ok(ApiClient {
            http,
            base,
            info,
            token: None,
        })
    }

    pub fn with_token(mut self, token: impl Into<String>) -> Self {
        self.token = Some(token.into());
        self
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    pub fn info(&self) -> &ServerInfo {
        &self.info
    }

    pub fn http(&self) -> &reqwest::Client {
        &self.http
    }

    fn api(&self, path: &str) -> String {
        format!("{}{}", self.info.api.trim_end_matches('/'), path)
    }

    fn request(&self, method: reqwest::Method, url: String) -> reqwest::RequestBuilder {
        let req = self.http.request(method, url);
        match &self.token {
            Some(token) => req.bearer_auth(token),
            None => req,
        }
    }

    /// `GET` an API path and decode JSON.
    pub async fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        let resp = self
            .request(reqwest::Method::GET, self.api(path))
            .send()
            .await
            .map_err(net)?;
        check(resp).await?.json().await.map_err(net)
    }

    /// Send JSON to an API path and decode the JSON reply.
    pub async fn send_json<B: Serialize, T: DeserializeOwned>(
        &self,
        method: reqwest::Method,
        path: &str,
        body: &B,
    ) -> Result<T> {
        let resp = self
            .request(method, self.api(path))
            .json(body)
            .send()
            .await
            .map_err(net)?;
        check(resp).await?.json().await.map_err(net)
    }

    async fn send_json_status<B: Serialize>(
        &self,
        method: reqwest::Method,
        path: &str,
        body: &B,
    ) -> Result<u16> {
        let resp = self
            .request(method, self.api(path))
            .json(body)
            .send()
            .await
            .map_err(net)?;
        Ok(check(resp).await?.status().as_u16())
    }

    // ---- public ----------------------------------------------------------

    pub async fn game(&self, slug: &str) -> Result<GameManifest> {
        let resp = self
            .http
            .get(format!("{}/games/{slug}/manifest.json", self.base))
            .send()
            .await
            .map_err(net)?;
        let text = check(resp).await?.text().await.map_err(net)?;
        let game = GameManifest::parse(&text)?;
        if game.slug != slug {
            return Err(Error::invalid(
                "game manifest",
                "server answered with a different slug",
            ));
        }
        Ok(game)
    }

    /// Raw, unverified release manifest text.
    pub async fn release_text(&self, slug: &str, version: &str) -> Result<String> {
        crate::ids::parse_version(version)?;
        let resp = self
            .http
            .get(format!(
                "{}/games/{slug}/releases/{version}.json",
                self.base
            ))
            .send()
            .await
            .map_err(net)?;
        check(resp).await?.text().await.map_err(net)
    }

    /// Fetch a release and verify it against `pinned`.
    pub async fn release(
        &self,
        slug: &str,
        version: &str,
        pinned: &PublicKey,
    ) -> Result<SignedRelease> {
        let text = self.release_text(slug, version).await?;
        let release = SignedRelease::parse_pinned(&text, pinned)?;
        let m = release.manifest();
        if m.slug != slug || m.version != version {
            return Err(Error::invalid(
                "release manifest",
                format!(
                    "asked for {slug} {version} but the server sent {} {}",
                    m.slug, m.version
                ),
            ));
        }
        Ok(release)
    }

    pub async fn releases(&self, slug: &str) -> Result<ReleaseList> {
        self.get_json(&format!("/games/{slug}/releases")).await
    }

    pub async fn catalog(&self, query: &str, page: u32) -> Result<CatalogPage> {
        let resp = self
            .http
            .get(self.api("/games"))
            .query(&[("q", query), ("page", &page.to_string())])
            .send()
            .await
            .map_err(net)?;
        check(resp).await?.json().await.map_err(net)
    }

    /// Download one chunk and verify it.
    pub async fn chunk(&self, id: &ChunkId) -> Result<Vec<u8>> {
        let resp = self
            .http
            .get(self.info.chunk_url(id))
            .send()
            .await
            .map_err(net)?;
        let data = check(resp).await?.bytes().await.map_err(net)?;
        let actual = ChunkId::of(&data);
        if actual != *id {
            return Err(Error::ChunkMismatch {
                id: id.to_hex(),
                actual: actual.to_hex(),
            });
        }
        Ok(data.to_vec())
    }

    // ---- developer -------------------------------------------------------

    pub async fn register(&self, email: &str, password: &str, invite: Option<&str>) -> Result<()> {
        self.send_json_status(
            reqwest::Method::POST,
            "/auth/register",
            &RegisterRequest {
                email: email.to_string(),
                password: password.to_string(),
                invite: invite.map(str::to_string),
            },
        )
        .await
        .map(|_| ())
    }

    pub async fn token(&self, email: &str, password: &str, name: Option<&str>) -> Result<String> {
        let resp: TokenResponse = self
            .send_json(
                reqwest::Method::POST,
                "/auth/token",
                &TokenRequest {
                    email: email.to_string(),
                    password: password.to_string(),
                    name: name.map(str::to_string),
                },
            )
            .await?;
        Ok(resp.token)
    }

    pub async fn me(&self) -> Result<Me> {
        self.get_json("/dev/me").await
    }

    pub async fn add_key(&self, pubkey: PublicKey, label: Option<&str>) -> Result<()> {
        self.send_json_status(
            reqwest::Method::POST,
            "/dev/keys",
            &AddKeyRequest {
                pubkey,
                label: label.map(str::to_string),
            },
        )
        .await
        .map(|_| ())
    }

    pub async fn create_game(&self, req: &CreateGameRequest) -> Result<GameManifest> {
        self.send_json(reqwest::Method::POST, "/games", req).await
    }

    pub async fn patch_game(&self, slug: &str, req: &PatchGameRequest) -> Result<GameManifest> {
        self.send_json(reqwest::Method::PATCH, &format!("/games/{slug}"), req)
            .await
    }

    pub async fn missing_chunks(&self, ids: &[ChunkId]) -> Result<Vec<ChunkId>> {
        let mut missing = Vec::new();
        for batch in ids.chunks(5000) {
            let resp: MissingChunksResponse = self
                .send_json(
                    reqwest::Method::POST,
                    "/chunks/missing",
                    &MissingChunksRequest {
                        ids: batch.to_vec(),
                    },
                )
                .await?;
            missing.extend(resp.missing);
        }
        Ok(missing)
    }

    /// Upload a chunk. Returns `true` if the server stored it, `false` if it
    /// already had it.
    pub async fn put_chunk(&self, id: &ChunkId, data: Vec<u8>) -> Result<bool> {
        let resp = self
            .request(reqwest::Method::PUT, self.api(&format!("/chunks/{id}")))
            .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
            .body(data)
            .send()
            .await
            .map_err(net)?;
        Ok(check(resp).await?.status() == reqwest::StatusCode::CREATED)
    }

    /// Publish a signed release. Returns `true` if newly created, `false` if
    /// the identical release was already published.
    pub async fn publish_release(&self, slug: &str, release: &SignedRelease) -> Result<bool> {
        let resp = self
            .request(
                reqwest::Method::POST,
                self.api(&format!("/games/{slug}/releases")),
            )
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(release.text().to_string())
            .send()
            .await
            .map_err(net)?;
        Ok(check(resp).await?.status() == reqwest::StatusCode::CREATED)
    }

    pub async fn set_channel(&self, slug: &str, channel: &str, version: &str) -> Result<()> {
        self.send_json_status(
            reqwest::Method::POST,
            &format!("/games/{slug}/channels"),
            &SetChannelRequest {
                channel: channel.to_string(),
                version: version.to_string(),
            },
        )
        .await
        .map(|_| ())
    }

    pub async fn post_rotation(&self, slug: &str, statement: &serde_json::Value) -> Result<()> {
        self.send_json_status(
            reqwest::Method::POST,
            &format!("/games/{slug}/rotations"),
            statement,
        )
        .await
        .map(|_| ())
    }

    pub async fn delete_release(&self, slug: &str, version: &str) -> Result<()> {
        let resp = self
            .request(
                reqwest::Method::DELETE,
                self.api(&format!("/games/{slug}/releases/{version}")),
            )
            .send()
            .await
            .map_err(net)?;
        check(resp).await.map(|_| ())
    }
}
