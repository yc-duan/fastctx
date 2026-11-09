//! Pure business rules over the registered data foundation. No storage or IO.
//!
//! Schema readiness is distinct from service activation. Contextual projections,
//! source ingestion and compound publication remain disabled until their real
//! foundation ports are installed. See [`registry`] for the closed intake list.
pub mod account;
pub mod context;
mod json;
pub mod registry;

pub use fastctx_sync::store::Error;
