//! Account-scoped sentinel services and their integration ports.
mod control;
pub(crate) mod deletion;
mod settings;
mod transport;
pub(crate) use control::{control, list_all, scope};
use fastctx_sentinel::{ports::Credentials, records::Records};
use fastctx_types::ipc::Failure;
pub(crate) use settings::settings;
use std::sync::{Arc, OnceLock};
pub(crate) use transport::home_call;

/// The account record and secret providers are attached by node initialization.
/// Until attached, callers receive an explicit unavailable result.
#[derive(Default)]
pub(crate) struct Services {
    pub records: OnceLock<Arc<dyn Records>>,
    pub credentials: OnceLock<Arc<dyn Credentials>>,
    pub host: OnceLock<Arc<dyn fastctx_sentinel::ports::ResidentHost>>,
}
impl Services {
    pub fn records(&self) -> Result<&dyn Records, Failure> {
        self.records
            .get()
            .map(AsRef::as_ref)
            .ok_or_else(|| Failure::not_run("sentinel account record service is not connected"))
    }
}
