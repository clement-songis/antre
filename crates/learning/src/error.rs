use thiserror::Error;

/// Errors of the learning loop.
#[derive(Debug, Error)]
pub enum Error {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid target: {0}")]
    InvalidTarget(String),
    #[error("change cannot apply: {0}")]
    Conflict(String),
    #[error("{0} changed since the proposal was made; reject it or ask for a new review")]
    StaleTarget(String),
    #[error("refused by the learning guard: {0}")]
    Refused(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("model error: {0}")]
    Model(String),
    #[error("could not parse the model response: {0}")]
    Response(String),
}

pub type Result<T> = std::result::Result<T, Error>;
