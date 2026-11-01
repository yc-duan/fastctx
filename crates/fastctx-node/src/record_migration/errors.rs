//! Quarantine is a content decision. An unreadable source remains retryable,
//! including IO errors carried through a streaming JSON deserializer.
use std::io;

pub(crate) fn damaged(error: &io::Error) -> bool {
    if super::files::storage_error(error) {
        return false;
    }
    if let Some(inner) = error.get_ref() {
        if let Some(json) = inner.downcast_ref::<serde_json::Error>() {
            return !json.is_io();
        }
        if let Some(nested) = inner.downcast_ref::<io::Error>() {
            return damaged(nested);
        }
    }
    error.kind() == io::ErrorKind::InvalidData
}

pub(crate) fn retry(error: io::Error) -> io::Error {
    fastctx_platform::readings::counter("migration.retryable_failures").add(1);
    if !super::files::storage_error(&error)
        && !matches!(error.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted)
    {
        fastctx_platform::readings::counter("migration.source_read_retries").add(1);
    }
    error
}

pub(crate) fn invalid(reason: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, reason)
}
