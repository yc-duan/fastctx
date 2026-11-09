//! Pure validation shared by the source writer and accepting authority.
//! Custody provenance is authenticated by the enclosing engine before this gate.
mod authorizer;
pub mod event;
mod roots;
pub use authorizer::TimelineAuthorizer;
pub use roots::direct_roots;
