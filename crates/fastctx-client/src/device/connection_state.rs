use fastctx_net::link::Status;
use fastctx_types::access::Written;

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

    /// The same for the link that reaches `machine`: the hub it is on, or
    /// the one hub when no machine is named.
    pub fn machine_changes(
        &self,
        machine: Option<&Written>,
    ) -> Result<(fastctx_types::HubName, tokio::sync::watch::Receiver<Status>), Error> {
        let session = match machine {
            Some(machine) => self.reaching(machine)?,
            None => self.on_hub(None)?,
        };
        Ok((session.link.hub_name(), session.link.subscribe()))
    }

    /// How many checks each hub has answered (`Link::checks`): a native
    /// application that asks for one on coming back to the front knows by
    /// the count when a link it kept was heard again.
    pub fn checks(&self) -> Vec<(fastctx_types::HubName, u64)> {
        self.sessions().iter().map(|session| (session.link.hub_name(), *session.link.checks().borrow())).collect()
    }

    /// The same count for the link that reaches `machine`, as it changes.
    pub fn machine_checks(&self, machine: Option<&Written>) -> Result<tokio::sync::watch::Receiver<u64>, Error> {
        let session = match machine {
            Some(machine) => self.reaching(machine)?,
            None => self.on_hub(None)?,
        };
        Ok(session.link.checks())
    }
}
