use thiserror::Error;

/// Errors produced by config loading, the store, and redaction helpers.
#[derive(Debug, Error)]
pub enum CoreError {
    #[error("io error at {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("suite config error: {0}")]
    Config(String),
    #[error("json error: {0}")]
    Json(String),
    #[error("run not found: {0}")]
    NotFound(String),
}

impl CoreError {
    pub fn io(path: impl Into<String>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}
