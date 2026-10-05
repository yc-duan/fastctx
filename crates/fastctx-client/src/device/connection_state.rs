use fastctx_net::link::Status;

use super::Device;
use crate::Error;

impl Device {
    /// A native application can stop live subscriptions when their link is
    /// replaced and read fresh history when it returns. Leaving or revoking
    /// this membership stops this receiver; another membership needs a new one.
    pub fn connection_changes(&self) -> Result<tokio::sync::watch::Receiver<Status>, Error> {
        Ok(self.connected()?.link.subscribe())
    }
}
