//! Pure business rules over the registered data foundation. No storage or IO.
//!
//! Schema readiness is distinct from service activation. Contextual projections,
//! source ingestion and compound publication remain disabled until their real
//! foundation ports are installed. The [`registry`] list is not an activation
//! gate: services must install the complete ready bundles from [`installation`],
//! including their object reader policies, before advertising a kind.
pub mod account;
pub mod context;
pub mod installation;
mod json;
pub mod registry;

pub use fastctx_sync::store::Error;
