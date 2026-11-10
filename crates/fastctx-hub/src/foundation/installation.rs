//! Policies arrive only from trusted in-process composition, never the wire.
use fastctx_sync::store::{access::AccessPolicy, source_authorize::SourceAuthorizer};
use std::sync::Arc;

/// Installed before the hub opens its foundation. A missing source kind remains
/// unavailable. The caller owns the policies' trusted state and its refreshes.
#[derive(Clone)]
pub struct Installation {
    pub access: Arc<dyn AccessPolicy>,
    pub sources: Vec<Arc<dyn SourceAuthorizer>>,
}
impl Default for Installation {
    fn default() -> Self {
        Self { access: Arc::new(super::policy::HubPolicy), sources: Vec::new() }
    }
}
