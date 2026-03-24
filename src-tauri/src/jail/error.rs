//! Unified error type for jail operations.

#[derive(Debug, thiserror::Error)]
pub enum JailError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("git error: {0}")]
    Git(String),

    #[error("CoW setup failed: {0}")]
    CowSetup(String),

    #[error("path encoding error: {0}")]
    PathEncoding(String),

    #[error("session link error: {0}")]
    SessionLink(String),
}
