use serde::{Deserialize, Serialize};

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(tag = "type")]
pub enum Error {
    #[error("multi-user sessions are not supported by this host OS")]
    MultiUserNotSupportedByHostOs,
    #[error("user not found")]
    UserNotFound,
    #[error("session not found")]
    SessionNotFound,
    #[error("permission denied")]
    PermissionDenied,
    #[error("backend unavailable: {detail}")]
    BackendUnavailable { detail: String },
    #[error("isolation violation")]
    IsolationViolation,
    #[error("protocol error")]
    Protocol,
    #[error("I/O error")]
    Io,
    #[error("authentication error")]
    Auth,
    #[error("unsupported")]
    Unsupported,
}

impl Error {
    pub fn backend(detail: impl Into<String>) -> Self {
        Self::BackendUnavailable {
            detail: detail.into(),
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(_: std::io::Error) -> Self {
        Self::Io
    }
}

impl From<serde_json::Error> for Error {
    fn from(_: serde_json::Error) -> Self {
        Self::Protocol
    }
}
