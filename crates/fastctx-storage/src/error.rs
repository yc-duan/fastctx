//! Stable error classes let callers distinguish admission from unknown outcomes.

use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    Busy,
    Corrupt,
    Full,
    Unavailable,
    OutcomeUnknown,
    Invalid,
    Other,
}

#[derive(Clone, Debug)]
pub struct Error {
    kind: ErrorKind,
    reason: Arc<str>,
    database_damage: bool,
}

impl Error {
    pub fn new(reason: impl Into<String>) -> Self {
        Self::with_kind(ErrorKind::Other, reason)
    }

    pub fn with_kind(kind: ErrorKind, reason: impl Into<String>) -> Self {
        Self { kind, reason: Arc::from(reason.into()), database_damage: false }
    }

    /// True only for SQLite physical integrity failures, never a service's
    /// malformed record or unknown business format.
    pub fn is_database_damage(&self) -> bool {
        self.database_damage
    }

    pub(crate) fn damaged(reason: impl Into<String>) -> Self {
        Self { kind: ErrorKind::Corrupt, reason: Arc::from(reason.into()), database_damage: true }
    }

    pub fn kind(&self) -> ErrorKind {
        self.kind
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.reason.fmt(f)
    }
}
impl std::error::Error for Error {}

impl From<rusqlite::Error> for Error {
    fn from(error: rusqlite::Error) -> Self {
        use rusqlite::ErrorCode;
        let kind = match error.sqlite_error_code() {
            Some(ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked) => ErrorKind::Busy,
            Some(ErrorCode::DatabaseCorrupt | ErrorCode::NotADatabase) => ErrorKind::Corrupt,
            Some(ErrorCode::DiskFull) => ErrorKind::Full,
            Some(ErrorCode::CannotOpen | ErrorCode::SystemIoFailure) => ErrorKind::Unavailable,
            Some(ErrorCode::ConstraintViolation | ErrorCode::TypeMismatch) => ErrorKind::Invalid,
            _ => ErrorKind::Other,
        };
        if kind == ErrorKind::Corrupt {
            Self::damaged(error.to_string())
        } else {
            Self::with_kind(kind, error.to_string())
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        let kind = match error.kind() {
            std::io::ErrorKind::StorageFull => ErrorKind::Full,
            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut => ErrorKind::Busy,
            std::io::ErrorKind::InvalidData | std::io::ErrorKind::InvalidInput => ErrorKind::Invalid,
            _ => ErrorKind::Unavailable,
        };
        Self::with_kind(kind, error.to_string())
    }
}
