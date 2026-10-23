//! Account-scoped sentinel services and their integration ports.
mod control;
pub(crate) mod goal;
pub(crate) mod goal_seams;
mod effects;
mod delivery;
mod worker;
pub(crate) use worker::{start,report};
pub(crate) mod deletion;
mod settings;
pub(crate) mod keys;
pub(crate) mod look;
mod records;
pub(crate) mod timers;
mod transport;
pub(crate) use control::{control, list_all, scope};
use fastctx_sentinel::{ports::Credentials, records::Records};
use fastctx_types::ipc::Failure;
pub(crate) use settings::settings;
use std::sync::{Arc, OnceLock};
pub(crate) use transport::home_call;

/// The account record, secret and timer providers are attached by `connected`.
/// Until attached, callers receive an explicit unavailable result.
#[derive(Default)]
pub(crate) struct Services {
    pub records: OnceLock<Arc<dyn Records>>,
    pub native_watches: std::sync::Mutex<std::collections::HashSet<fastctx_types::ResidentId>>,
    pub credentials: OnceLock<Arc<dyn Credentials>>,
    pub timers: OnceLock<Arc<dyn fastctx_sentinel::ports::Timers>>,
    /// Serializes changes to the keys this machine holds with the settings read that allows them.
    pub keys: tokio::sync::Mutex<()>,
    firings: timers::Firings,
    worker: worker::Worker,
}
impl Services {
    pub fn records(&self) -> Result<&dyn Records, Failure> {
        self.records
            .get()
            .map(AsRef::as_ref)
            .ok_or_else(|| Failure::not_run("sentinel account record service is not connected"))
    }
}

/// The services of a node, on its account records, key store and timer table.
pub(crate) fn connected(node: std::sync::Weak<crate::Node>) -> Services {
    let services = Services::default();
    let _ = services.records.set(Arc::new(records::AccountRecords(node.clone())));
    let _ = services.credentials.set(Arc::new(keys::Keystore(node.clone())));
    let _ = services.timers.set(Arc::new(timers::SentinelTimers(node)));
    services
}
