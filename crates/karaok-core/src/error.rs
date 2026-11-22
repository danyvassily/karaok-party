use thiserror::Error;

/// Shared error type for domain operations in `karaok-core`.
#[derive(Debug, Error)]
pub enum KaraokError {
    #[error("Invalid lyrics format: {0}")]
    InvalidLyrics(String),

    #[error("Invalid musical key: {0}")]
    InvalidKey(String),

    #[error("Invalid parameter '{name}': {reason}")]
    InvalidParameter { name: &'static str, reason: String },

    #[error("Resource not found: {0}")]
    NotFound(String),
}

pub type Result<T> = std::result::Result<T, KaraokError>;
