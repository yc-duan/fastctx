//! Fixed-width SQL values preserve unsigned ordering and durable error classes.
use super::Error;
use crate::identity::OperationId;
use fastctx_storage::{
    ErrorKind,
    rusqlite::{self, types::Type},
};

pub(crate) fn counter(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<u64> {
    let bytes: Vec<u8> = row.get(index)?;
    let bytes: [u8; 8] =
        bytes.try_into().map_err(|_| rusqlite::Error::InvalidColumnType(index, "counter".into(), Type::Blob))?;
    Ok(u64::from_be_bytes(bytes))
}

pub(crate) fn invalid(reason: impl Into<String>) -> fastctx_storage::Error {
    fastctx_storage::Error::with_kind(ErrorKind::Invalid, reason)
}

pub(crate) fn error(error: fastctx_storage::Error, operation: OperationId) -> Error {
    match error.kind() {
        ErrorKind::Busy => Error::Busy,
        ErrorKind::Corrupt => Error::Corrupt(error.to_string()),
        ErrorKind::Full => Error::Full,
        ErrorKind::Invalid => Error::Invalid(error.to_string()),
        ErrorKind::OutcomeUnknown => Error::OutcomeUnknown { operation },
        ErrorKind::Other | ErrorKind::Unavailable => Error::Unavailable(error.to_string()),
    }
}
