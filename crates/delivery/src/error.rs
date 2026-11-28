#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("delivery store: {0}")]
    Database(#[from] sqlx::Error),
    #[error("delivery migrations: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),
    #[error("delivery record: {0}")]
    Json(#[from] serde_json::Error),
    #[error("corrupt delivery record: {0}")]
    Corrupt(String),
}

pub type Result<T> = std::result::Result<T, Error>;
