use thiserror::Error;

/// Errors from event validation, import, pricing, and aggregation.
/// Per-event problems never abort a batch: they land in the import
/// summary with line numbers and reasons.
#[derive(Debug, Error)]
pub enum UsageError {
    #[error("io error at {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("event rejected (line {line}): {reason}")]
    Rejected { line: usize, reason: String },
    #[error("unsupported event version {0} (this reader supports llm/v{max})", max = crate::event::EVENT_VERSION)]
    UnsupportedVersion(u32),
    #[error("config error: {0}")]
    Config(String),
}

impl UsageError {
    pub fn io(path: impl Into<String>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}
