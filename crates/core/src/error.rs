use std::path::PathBuf;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid {what}: {reason}")]
    Invalid { what: &'static str, reason: String },

    #[error("unsafe path {path:?}: {reason}")]
    UnsafePath { path: String, reason: &'static str },

    #[error("signature verification failed")]
    BadSignature,

    #[error("schema validation failed: {0}")]
    Schema(String),

    #[error("chunk {id} failed verification (got {actual})")]
    ChunkMismatch { id: String, actual: String },

    #[error("chunk {0} is not in the content store")]
    MissingChunk(String),

    #[error("{context}: {source}")]
    Io {
        context: String,
        #[source]
        source: std::io::Error,
    },

    #[error("json: {0}")]
    Json(#[from] serde_json::Error),

    #[error("server returned {status}: {title}{}", if .detail.is_empty() { String::new() } else { format!(" — {}", .detail) })]
    Api {
        status: u16,
        title: String,
        detail: String,
    },

    #[error("network: {0}")]
    Network(String),

    #[error("install failed and was rolled back: {0}")]
    RolledBack(String),

    #[error("{0}")]
    Other(String),
}

impl Error {
    pub fn invalid(what: &'static str, reason: impl Into<String>) -> Self {
        Error::Invalid {
            what,
            reason: reason.into(),
        }
    }

    pub fn io(context: impl Into<String>, source: std::io::Error) -> Self {
        Error::Io {
            context: context.into(),
            source,
        }
    }

    pub fn other(msg: impl Into<String>) -> Self {
        Error::Other(msg.into())
    }
}

/// Attach a path to an `io::Error`.
pub(crate) trait IoContext<T> {
    fn at(self, path: impl Into<PathBuf>) -> Result<T>;
}

impl<T> IoContext<T> for std::io::Result<T> {
    fn at(self, path: impl Into<PathBuf>) -> Result<T> {
        self.map_err(|e| Error::io(path.into().display().to_string(), e))
    }
}
