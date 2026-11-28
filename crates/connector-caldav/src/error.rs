use thiserror::Error;

pub type Result<T> = std::result::Result<T, CalDavConnectorError>;

#[derive(Debug, Error)]
pub enum CalDavConnectorError {
    #[error("invalid CalDAV account configuration: {0}")]
    AccountConfig(&'static str),
    #[error("invalid CalDAV dataset configuration: {0}")]
    DatasetConfig(&'static str),
    #[error("CalDAV server failed network safety validation")]
    NetworkSafety(#[source] antre_common::Error),
    #[error("CalDAV operation failed")]
    Client(#[source] antre_caldav::Error),
    #[error("invalid CalDAV server response: {0}")]
    ServerResponse(String),
    #[error("failed to encode connector item")]
    Serialization(#[source] serde_json::Error),
}

impl From<antre_caldav::Error> for CalDavConnectorError {
    fn from(error: antre_caldav::Error) -> Self {
        Self::Client(error)
    }
}

impl From<serde_json::Error> for CalDavConnectorError {
    fn from(error: serde_json::Error) -> Self {
        Self::Serialization(error)
    }
}
