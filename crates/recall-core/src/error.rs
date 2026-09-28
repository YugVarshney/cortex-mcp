//! Error type shared across the workspace.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum RecallError {
    #[error("namespace not found: {0}")]
    NamespaceNotFound(String),
    #[error("memory not found: {0}")]
    MemoryNotFound(String),
    #[error("namespace already exists: {0}")]
    DuplicateNamespace(String),
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("embedder error: {0}")]
    Embedder(String),
    #[error("encryption error: {0}")]
    Crypto(String),
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, RecallError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_messages_are_human_readable() {
        assert_eq!(
            RecallError::NamespaceNotFound("w".into()).to_string(),
            "namespace not found: w"
        );
        assert_eq!(
            RecallError::MemoryNotFound("id1".into()).to_string(),
            "memory not found: id1"
        );
        assert_eq!(
            RecallError::DuplicateNamespace("w".into()).to_string(),
            "namespace already exists: w"
        );
        assert_eq!(
            RecallError::InvalidInput("bad".into()).to_string(),
            "invalid input: bad"
        );
        assert_eq!(
            RecallError::Embedder("boom".into()).to_string(),
            "embedder error: boom"
        );
        assert_eq!(
            RecallError::Crypto("bad key".into()).to_string(),
            "encryption error: bad key"
        );
        let io_err: RecallError = std::io::Error::other("nope").into();
        assert!(io_err.to_string().contains("nope"));
        let ser: RecallError = serde_json::from_str::<String>("{").unwrap_err().into();
        assert!(ser.to_string().starts_with("serialization error"));
    }
}
