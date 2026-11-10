//! What refuses a write or a read of the content database.

use fastctx_storage::ErrorKind;

#[derive(Clone, Debug)]
pub enum Error {
    /// The request cannot be done as asked: too large, malformed, or of a
    /// generation the stream has left.
    Invalid(String),
    /// The database refused or failed; `ErrorKind::OutcomeUnknown` leaves a
    /// write's effect unknown.
    Storage(fastctx_storage::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(reason) => reason.fmt(f),
            Self::Storage(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for Error {}

impl From<fastctx_storage::Error> for Error {
    fn from(error: fastctx_storage::Error) -> Self {
        match error.kind() {
            ErrorKind::Invalid => Self::Invalid(error.to_string()),
            _ => Self::Storage(error),
        }
    }
}

/// A refusal raised inside a write, which the storage carries out to its caller.
pub(crate) fn invalid(reason: impl Into<String>) -> fastctx_storage::Error {
    fastctx_storage::Error::with_kind(ErrorKind::Invalid, reason)
}
