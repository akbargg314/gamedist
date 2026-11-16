//! Progress reporting shared by the download engine and installers.

use serde::Serialize;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    /// Recovering chunks from the installed version.
    Harvest,
    Download,
    Assemble,
    Verify,
    Done,
}

/// Where the bytes of a [`Phase::Download`] report come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    /// A server's chunk endpoint (or a mirror, or a CDN in front of one).
    Http,
    /// Other players, over BitTorrent.
    Peers,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Progress {
    pub phase: Phase,
    pub done: u64,
    pub total: u64,
    /// Set for [`Phase::Download`]; `done` and `total` count this source only.
    pub source: Option<Source>,
}

impl Progress {
    /// A report for work that happens on the local disk.
    pub fn local(phase: Phase, done: u64, total: u64) -> Self {
        Progress {
            phase,
            done,
            total,
            source: None,
        }
    }

    /// A download report for bytes arriving from `source`.
    pub fn download(source: Source, done: u64, total: u64) -> Self {
        Progress {
            phase: Phase::Download,
            done,
            total,
            source: Some(source),
        }
    }
}

/// Callback invoked from worker tasks; must be cheap and non-blocking.
pub type ProgressFn = Arc<dyn Fn(Progress) + Send + Sync>;

pub fn no_progress() -> ProgressFn {
    Arc::new(|_| {})
}
