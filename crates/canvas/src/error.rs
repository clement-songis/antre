#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("design not found: {0}")]
    NotFound(String),

    #[error("version {version} not found for design {design_id}")]
    VersionNotFound { design_id: String, version: u32 },

    #[error("invalid design id '{0}'")]
    InvalidId(String),

    #[error("invalid path '{path}': {reason}")]
    InvalidPath { path: String, reason: &'static str },

    #[error("file too large: {path} is {bytes} bytes (limit {limit})")]
    TooLarge {
        path: String,
        bytes: u64,
        limit: u64,
    },

    #[error("invalid request: {0}")]
    Invalid(String),

    #[error("{0} is not available")]
    Unavailable(&'static str),

    #[error("render failed: {0}")]
    Render(String),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Database(#[from] sqlx::Error),

    #[error(transparent)]
    Migrate(#[from] sqlx::migrate::MigrateError),

    #[error(transparent)]
    Json(#[from] serde_json::Error),

    #[error("checkpoint error: {0}")]
    Checkpoint(String),

    #[error("archive error: {0}")]
    Zip(#[from] zip::result::ZipError),

    #[error("{message}")]
    Message { message: String },
}

impl Error {
    pub fn message(message: impl Into<String>) -> Self {
        Self::Message {
            message: message.into(),
        }
    }

    /// Whether the error comes from the caller (bad id, path or parameters)
    /// rather than from the server.
    #[must_use]
    pub fn is_client_error(&self) -> bool {
        matches!(
            self,
            Self::NotFound(_)
                | Self::VersionNotFound { .. }
                | Self::InvalidId(_)
                | Self::InvalidPath { .. }
                | Self::TooLarge { .. }
                | Self::Invalid(_)
        )
    }

    /// Whether the error means the requested design, version or file is absent.
    #[must_use]
    pub fn is_not_found(&self) -> bool {
        matches!(self, Self::NotFound(_) | Self::VersionNotFound { .. })
            || matches!(self, Self::Io(io) if io.kind() == std::io::ErrorKind::NotFound)
    }
}

impl From<moltis_tools::Error> for Error {
    fn from(error: moltis_tools::Error) -> Self {
        Self::Checkpoint(error.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;
