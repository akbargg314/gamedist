//! Server configuration: TOML file with `GAMEDIST_*` environment overrides.

use gamedist_core::manifest::Registration;
use serde::Deserialize;
use std::{net::SocketAddr, path::PathBuf};

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Human-readable server name shown in the portal and server info.
    pub name: String,
    /// Address to listen on.
    pub listen: SocketAddr,
    /// Externally visible base URL, no trailing slash. Derived from `listen`
    /// when empty (fine for local testing only).
    pub public_url: String,
    /// Directory for the database and chunk storage.
    pub data_dir: PathBuf,
    /// Initial registration mode; afterwards the admin setting wins.
    pub registration: Registration,
    /// Initial per-account storage quota in bytes (0 = unlimited).
    pub default_quota_bytes: u64,
    /// Serve chunks from this URL prefix instead (CDN in front of `/chunks/`).
    pub chunks_url: Option<String>,
    /// Expose the public catalog (`/api/v0/games`, landing page listing).
    pub catalog: bool,
    /// Run the built-in HTTP tracker at `/announce`.
    pub tracker: bool,
    /// Extra trackers to put into generated torrents.
    pub external_trackers: Vec<String>,
    /// Tracker announce interval in seconds.
    pub tracker_interval_secs: u32,
    /// Trust `X-Forwarded-For` (set when running behind a reverse proxy).
    pub trust_proxy: bool,
    /// Login/registration attempts allowed per IP per minute.
    pub auth_rate_per_minute: u32,
    /// Chunk uploads allowed per IP per minute.
    pub upload_rate_per_minute: u32,
    /// Unreferenced chunks older than this are garbage-collected.
    pub gc_grace_days: u32,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            name: "GameDist server".to_string(),
            listen: "127.0.0.1:8080".parse().expect("valid address"),
            public_url: String::new(),
            data_dir: PathBuf::from("data"),
            registration: Registration::Closed,
            default_quota_bytes: 20 * 1024 * 1024 * 1024,
            chunks_url: None,
            catalog: true,
            tracker: true,
            external_trackers: Vec::new(),
            tracker_interval_secs: 120,
            trust_proxy: false,
            auth_rate_per_minute: 20,
            upload_rate_per_minute: 6000,
            gc_grace_days: 7,
        }
    }
}

impl Config {
    /// Load from an optional TOML file, then apply environment overrides.
    pub fn load(path: Option<&std::path::Path>) -> Result<Self, String> {
        let mut cfg = match path {
            Some(path) => {
                let text = std::fs::read_to_string(path)
                    .map_err(|e| format!("reading {}: {e}", path.display()))?;
                toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?
            }
            None => Config::default(),
        };
        cfg.apply_env(|key| std::env::var(key).ok())?;
        cfg.normalize()?;
        Ok(cfg)
    }

    fn apply_env(&mut self, get: impl Fn(&str) -> Option<String>) -> Result<(), String> {
        fn parse<T: std::str::FromStr>(key: &str, value: String) -> Result<T, String> {
            value
                .parse()
                .map_err(|_| format!("{key}: cannot parse {value:?}"))
        }
        if let Some(v) = get("GAMEDIST_NAME") {
            self.name = v;
        }
        if let Some(v) = get("GAMEDIST_LISTEN") {
            self.listen = parse("GAMEDIST_LISTEN", v)?;
        }
        if let Some(v) = get("GAMEDIST_PUBLIC_URL") {
            self.public_url = v;
        }
        if let Some(v) = get("GAMEDIST_DATA_DIR") {
            self.data_dir = PathBuf::from(v);
        }
        if let Some(v) = get("GAMEDIST_REGISTRATION") {
            self.registration = Registration::parse(&v).map_err(|e| e.to_string())?;
        }
        if let Some(v) = get("GAMEDIST_DEFAULT_QUOTA_BYTES") {
            self.default_quota_bytes = parse("GAMEDIST_DEFAULT_QUOTA_BYTES", v)?;
        }
        if let Some(v) = get("GAMEDIST_CHUNKS_URL") {
            self.chunks_url = Some(v);
        }
        if let Some(v) = get("GAMEDIST_CATALOG") {
            self.catalog = parse("GAMEDIST_CATALOG", v)?;
        }
        if let Some(v) = get("GAMEDIST_TRACKER") {
            self.tracker = parse("GAMEDIST_TRACKER", v)?;
        }
        if let Some(v) = get("GAMEDIST_TRUST_PROXY") {
            self.trust_proxy = parse("GAMEDIST_TRUST_PROXY", v)?;
        }
        Ok(())
    }

    pub(crate) fn normalize(&mut self) -> Result<(), String> {
        self.public_url = self.public_url.trim_end_matches('/').to_string();
        if !self.public_url.is_empty()
            && !(self.public_url.starts_with("https://") || self.public_url.starts_with("http://"))
        {
            return Err("public_url must start with https:// or http://".into());
        }
        if let Some(chunks) = &mut self.chunks_url {
            if !chunks.ends_with('/') {
                chunks.push('/');
            }
        }
        Ok(())
    }

    pub fn chunks_base(&self) -> String {
        self.chunks_url
            .clone()
            .unwrap_or_else(|| format!("{}/chunks/", self.public_url))
    }

    pub fn tracker_url(&self) -> Option<String> {
        self.tracker
            .then(|| format!("{}/announce", self.public_url))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toml_and_env() {
        let mut cfg: Config = toml::from_str(
            r#"
            name = "My Server"
            listen = "0.0.0.0:9000"
            public_url = "https://games.example/"
            registration = "invite"
            chunks_url = "https://cdn.example/chunks"
            "#,
        )
        .unwrap();
        cfg.apply_env(|k| (k == "GAMEDIST_REGISTRATION").then(|| "open".to_string()))
            .unwrap();
        cfg.normalize().unwrap();
        assert_eq!(cfg.public_url, "https://games.example");
        assert_eq!(cfg.registration, Registration::Open);
        assert_eq!(cfg.chunks_base(), "https://cdn.example/chunks/");
        assert_eq!(
            cfg.tracker_url().as_deref(),
            Some("https://games.example/announce")
        );
    }

    #[test]
    fn rejects_unknown_keys_and_bad_values() {
        assert!(toml::from_str::<Config>("nmae = \"typo\"").is_err());
        let mut cfg = Config::default();
        assert!(cfg
            .apply_env(|k| (k == "GAMEDIST_LISTEN").then(|| "nope".to_string()))
            .is_err());
        cfg.public_url = "ftp://x".into();
        assert!(cfg.normalize().is_err());
    }
}
