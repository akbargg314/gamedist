//! GameDist distributor server.
//!
//! One binary serves the REST API, chunk storage, generated torrents with a
//! built-in tracker, and the server-rendered web portal.

pub mod api;
pub mod auth;
pub mod config;
pub mod db;
pub mod error;
pub mod games;
pub mod jobs;
pub mod metrics;
pub mod portal;
pub mod public;
pub mod state;
pub mod torrents;
pub mod tracker;

use axum::{
    extract::DefaultBodyLimit,
    routing::{get, post},
    Router,
};
pub use config::Config;
use gamedist_core::store::ContentStore;
use state::{AppState, St};
use std::{net::SocketAddr, sync::Arc};

/// Largest request bodies accepted.
const CHUNK_BODY_LIMIT: usize = 8 * 1024 * 1024 + 1024;
const MANIFEST_BODY_LIMIT: usize = 64 * 1024 * 1024;

pub fn router(st: St) -> Router {
    let api = Router::new()
        // public
        .route("/games", get(public::catalog).post(api::create_game))
        .route("/export.jsonl", get(public::export))
        .route(
            "/games/{slug}",
            get(public::game_manifest).patch(api::patch_game),
        )
        .route(
            "/games/{slug}/releases",
            get(public::release_list)
                .post(api::publish_release)
                .layer(DefaultBodyLimit::max(MANIFEST_BODY_LIMIT)),
        )
        .route(
            "/games/{slug}/releases/{version}",
            get(public::release_api).delete(api::delete_release),
        )
        .route("/games/{slug}/channels", post(api::set_channel))
        .route("/games/{slug}/rotations", post(api::post_rotation))
        .route("/chunks/missing", post(api::missing_chunks))
        .route(
            "/chunks/{id}",
            get(public::chunk)
                .put(api::put_chunk)
                .layer(DefaultBodyLimit::max(CHUNK_BODY_LIMIT)),
        )
        // developer
        .route("/auth/register", post(api::register))
        .route("/auth/token", post(api::token))
        .route("/dev/me", get(api::me))
        .route("/dev/keys", post(api::add_key))
        // admin
        .route(
            "/admin/users",
            get(api::admin_users).post(api::admin_update_user),
        )
        .route("/admin/games", get(api::admin_games))
        .route(
            "/admin/takedowns",
            get(api::admin_takedowns).post(api::admin_takedown),
        )
        .route(
            "/admin/settings",
            get(api::admin_settings).post(api::admin_update_settings),
        )
        .route("/admin/invites", post(api::admin_create_invite))
        .route("/admin/audit", get(api::admin_audit));

    Router::new()
        .route("/.well-known/gamedist.json", get(public::server_info))
        .route("/games/{slug}/manifest.json", get(public::game_manifest))
        .route("/games/{slug}/releases/", get(public::release_list))
        .route("/games/{slug}/releases/{file}", get(public::release_file))
        .route(
            "/games/{slug}/releases/{version}/{platform}/files/{*path}",
            get(public::web_seed_file),
        )
        .route("/chunks/{id}", get(public::chunk))
        .route("/announce", get(tracker::announce))
        .route("/metrics", get(metrics::handler))
        .nest("/api/v0", api)
        .merge(portal::routes())
        .with_state(st)
}

/// A bound, not-yet-running server.
pub struct Server {
    pub state: St,
    pub addr: SocketAddr,
    listener: tokio::net::TcpListener,
}

impl Server {
    /// Open the database and storage, bind the listener.
    pub async fn bind(mut cfg: Config) -> Result<Self, String> {
        cfg.normalize()?;
        std::fs::create_dir_all(&cfg.data_dir)
            .map_err(|e| format!("creating {}: {e}", cfg.data_dir.display()))?;
        let listener = tokio::net::TcpListener::bind(cfg.listen)
            .await
            .map_err(|e| format!("binding {}: {e}", cfg.listen))?;
        let addr = listener.local_addr().map_err(|e| e.to_string())?;
        if cfg.public_url.is_empty() {
            cfg.public_url = format!("http://{addr}");
        }
        let db = db::open(&cfg.data_dir.join("gamedist.sqlite"))
            .await
            .map_err(|e| format!("opening database: {e}"))?;
        let chunks = ContentStore::open(cfg.data_dir.join("chunks")).map_err(|e| e.to_string())?;
        let state = Arc::new(AppState::new(cfg, db, chunks));
        Ok(Server {
            state,
            addr,
            listener,
        })
    }

    pub fn url(&self) -> &str {
        &self.state.cfg.public_url
    }

    /// Serve until `shutdown` resolves.
    pub async fn run(
        self,
        shutdown: impl std::future::Future<Output = ()> + Send + 'static,
    ) -> std::io::Result<()> {
        let jobs = tokio::spawn(jobs::run(self.state.clone()));
        let app = router(self.state.clone());
        let result = axum::serve(
            self.listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(shutdown)
        .await;
        jobs.abort();
        result
    }
}
