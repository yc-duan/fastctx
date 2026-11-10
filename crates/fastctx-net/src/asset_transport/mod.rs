//! Authenticated asset data sockets carry bounded metadata and single-block IO.
mod answers;
pub(crate) mod channel;
pub mod client;
pub(crate) mod parts;
pub mod server;
pub mod sessions;
pub use channel::{Channel, frame_bytes};

use fastctx_sync::store::Error;
use fastctx_types::sync::session::{TransferError, TransferErrorCode};

pub(crate) fn io_error(error: impl std::fmt::Display) -> Error {
    Error::Unavailable(error.to_string())
}
pub(crate) fn remote_error(error: TransferError) -> Error {
    match error.code {
        TransferErrorCode::Busy => Error::Busy,
        TransferErrorCode::DiskFull => Error::Full,
        TransferErrorCode::NotAccessible => Error::Denied(error.detail),
        TransferErrorCode::Revoked => Error::Revoked,
        TransferErrorCode::InvalidRequest | TransferErrorCode::TooLarge => Error::Invalid(error.detail),
        TransferErrorCode::CorruptContent
        | TransferErrorCode::CorruptFrame
        | TransferErrorCode::CorruptRepresentation => Error::Corrupt(error.detail),
        _ => Error::Unavailable(error.detail),
    }
}
