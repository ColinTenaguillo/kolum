use crate::ports::Capability;

/// What can go wrong talking to a provider, in terms the UI can act on:
/// `Unauthorized` means "check your token", `RateLimited` means "wait",
/// and `Unsupported` should never reach the user because the UI hides
/// what `Capabilities` says is missing.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum DomainError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("unauthorized: {0}")]
    Unauthorized(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("rate limited, retry in {retry_after_secs:?}s")]
    RateLimited { retry_after_secs: Option<u64> },
    #[error("the provider does not support {0:?}")]
    Unsupported(Capability),
    #[error("transport: {0}")]
    Transport(String),
}

pub type Result<T> = std::result::Result<T, DomainError>;
