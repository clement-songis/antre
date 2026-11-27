//! Catalog errors.

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid catalog JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid catalog TOML: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("catalog I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("catalog download failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("catalog download returned HTTP {0}")]
    HttpStatus(u16),
    #[error("catalog download exceeded {0} bytes")]
    TooLarge(usize),
    #[error("catalog contains no models")]
    Empty,
    #[error("unsupported rate-limit timezone '{0}': use a fixed offset like +08:00")]
    InvalidTimezone(String),
}

pub type Result<T> = std::result::Result<T, Error>;
