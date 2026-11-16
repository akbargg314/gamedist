//! Prometheus text-format metrics.

use crate::state::St;
use axum::{extract::State, http::header, response::IntoResponse};
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Default)]
pub struct Metrics {
    pub chunks_served: AtomicU64,
    pub chunk_bytes_served: AtomicU64,
    pub chunks_uploaded: AtomicU64,
    pub chunk_bytes_uploaded: AtomicU64,
    pub file_bytes_served: AtomicU64,
    pub releases_published: AtomicU64,
    pub announces: AtomicU64,
    pub auth_failures: AtomicU64,
}

impl Metrics {
    pub fn add(counter: &AtomicU64, n: u64) {
        counter.fetch_add(n, Ordering::Relaxed);
    }
}

pub async fn handler(State(st): State<St>) -> impl IntoResponse {
    let m = &st.metrics;
    let mut out = String::new();
    let mut counter = |name: &str, help: &str, value: u64| {
        out.push_str(&format!(
            "# HELP {name} {help}\n# TYPE {name} counter\n{name} {value}\n"
        ));
    };
    let get = |c: &AtomicU64| c.load(Ordering::Relaxed);
    counter(
        "gamedist_chunks_served_total",
        "Chunks served over HTTP.",
        get(&m.chunks_served),
    );
    counter(
        "gamedist_chunk_bytes_served_total",
        "Chunk bytes served over HTTP.",
        get(&m.chunk_bytes_served),
    );
    counter(
        "gamedist_file_bytes_served_total",
        "Bytes served through the web-seed file layout.",
        get(&m.file_bytes_served),
    );
    counter(
        "gamedist_chunks_uploaded_total",
        "Chunks accepted from developers.",
        get(&m.chunks_uploaded),
    );
    counter(
        "gamedist_chunk_bytes_uploaded_total",
        "Chunk bytes accepted from developers.",
        get(&m.chunk_bytes_uploaded),
    );
    counter(
        "gamedist_releases_published_total",
        "Releases published.",
        get(&m.releases_published),
    );
    counter(
        "gamedist_tracker_announces_total",
        "Tracker announces handled.",
        get(&m.announces),
    );
    counter(
        "gamedist_auth_failures_total",
        "Failed logins.",
        get(&m.auth_failures),
    );

    let mut gauge = |name: &str, help: &str, value: i64| {
        out.push_str(&format!(
            "# HELP {name} {help}\n# TYPE {name} gauge\n{name} {value}\n"
        ));
    };
    for (name, help, sql) in [
        (
            "gamedist_users",
            "Registered accounts.",
            "SELECT COUNT(*) FROM users",
        ),
        (
            "gamedist_games",
            "Registered games.",
            "SELECT COUNT(*) FROM games",
        ),
        (
            "gamedist_releases",
            "Published releases.",
            "SELECT COUNT(*) FROM releases",
        ),
        (
            "gamedist_chunks",
            "Stored chunks.",
            "SELECT COUNT(*) FROM chunks",
        ),
        (
            "gamedist_chunk_bytes",
            "Bytes of stored chunks.",
            "SELECT COALESCE(SUM(size), 0) FROM chunks",
        ),
    ] {
        if let Ok(value) = sqlx::query_scalar::<_, i64>(sql).fetch_one(&st.db).await {
            gauge(name, help, value);
        }
    }
    gauge(
        "gamedist_tracker_peers",
        "Peers currently known to the tracker.",
        st.tracker.peer_count() as i64,
    );
    ([(header::CONTENT_TYPE, "text/plain; version=0.0.4")], out)
}
