use crate::id::ModId;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid id `{value}`: {reason}")]
    InvalidId { value: String, reason: &'static str },

    #[error("invalid manifest: {0}")]
    Manifest(String),

    #[error("failed to parse manifest: {0}")]
    ManifestToml(#[from] toml::de::Error),

    #[error("package rejected: {0}")]
    Package(String),

    #[error("mod `{0}` is not installed")]
    NotInstalled(ModId),

    #[error("consent required for mod `{mod_id}`: approve consent hash {consent_hash}")]
    ConsentRequired { mod_id: ModId, consent_hash: String },

    #[error("consent hash mismatch for mod `{mod_id}`: expected {expected}, got {actual}")]
    ConsentMismatch {
        mod_id: ModId,
        expected: String,
        actual: String,
    },

    #[error("capability denied: {0}")]
    Denied(String),

    #[error("contribution `{0}` is already registered")]
    DuplicateContribution(String),

    #[error("wasm: {0}")]
    Wasm(String),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
