//! Non-content activity facts, distributed only as part of the signed inventory.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct NodeActivity {
    /// Whether the node owns a live control-center activity registry.
    pub(crate) engine_hosted: bool,
    /// Open local agent transport connections, including their initial handshake.
    pub(crate) agent_connections: usize,
    /// Requests in progress at the local control center.
    pub(crate) local_requests: usize,
    /// Calls executing for other members.
    pub(crate) remote_calls: usize,
    /// When these counters were observed; independent of hardware inventory collection.
    pub(crate) observed_at: String,
}

impl NodeActivity {
    /// Activity is republished at least every minute; a 90-second horizon allows delivery
    /// delay without presenting a disconnected machine's old counters as current usage.
    pub(crate) fn is_fresh(&self) -> bool {
        crate::world::parse_rfc3339(&self.observed_at).is_ok_and(|observed| {
            (-5..=90).contains(&(time::OffsetDateTime::now_utc() - observed).whole_seconds())
        })
    }

    pub(crate) fn same_counts(&self, other: &Self) -> bool {
        self.engine_hosted == other.engine_hosted
            && self.agent_connections == other.agent_connections
            && self.local_requests == other.local_requests
            && self.remote_calls == other.remote_calls
    }
}
