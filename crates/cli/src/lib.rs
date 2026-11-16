//! `gamedist` developer CLI, as a library so the commands can be driven from
//! integration tests and other tools.

pub mod commands;
pub mod config;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Core(#[from] gamedist_core::Error),
    #[error("{0}")]
    Message(String),
}

impl Error {
    pub fn msg(message: impl Into<String>) -> Self {
        Error::Message(message.into())
    }
}
