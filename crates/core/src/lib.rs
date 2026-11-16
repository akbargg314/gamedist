//! GameDist protocol core.
//!
//! Everything shared by the server, the developer CLI and the launcher lives
//! here: identifiers, canonical JSON, signing, manifests, content-defined
//! chunking, the content store, install/update assembly, torrent metainfo and
//! (behind the `net` feature) the HTTP API client and download engine.
//!
//! This crate deliberately has no UI or server dependencies.

pub mod api;
pub mod build;
pub mod chunker;
pub mod error;
pub mod ids;
pub mod install;
pub mod jcs;
pub mod keys;
pub mod manifest;
pub mod paths;
pub mod progress;
pub mod schema;
pub mod store;
pub mod torrent;

#[cfg(feature = "net")]
pub mod client;
#[cfg(feature = "net")]
pub mod download;

pub use error::{Error, Result};
pub use ids::{game_id, ChunkId, GameUrl};
pub use keys::{PublicKey, SecretKey};

/// Protocol identifier carried by every object.
pub const PROTOCOL: &str = "gamedist/0";

/// Largest chunk a consumer accepts (8 MiB).
pub const MAX_CHUNK_SIZE: u64 = 8 * 1024 * 1024;

/// Current UTC time in the `created` format used by manifests.
pub fn now_rfc3339() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}
