use fastctx_net::link::Status;

use super::Device;
use crate::Error;

impl Device {
    /// A native application can stop live subscriptions when the link of
    /// the hub `hub`, or of the one hub, is replaced and read fresh history
    /// when it returns. Logging out of that hub stops this receiver.
    pub fn connection_changes(
        &self,
        hub: Option<&fastctx_types::HubName>,
    ) -> Result<tokio::sync::watch::Receiver<Status>, Error> {
        Ok(self.on_hub(hub)?.link.subscribe())
    }
}
