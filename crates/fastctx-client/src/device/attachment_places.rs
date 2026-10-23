//! Choose storage on a held machine before a new conversation has a host.

use fastctx_types::access::{HubAccount, Purpose, Written};
use fastctx_types::attachments::AttachmentPlaces;

use super::{Device, failed};
use crate::Error;

impl Device {
    pub(super) fn unattached_places(
        &self,
        selected: Option<&HubAccount>,
        preferred: Option<&Written>,
    ) -> Result<AttachmentPlaces, Error> {
        let mut identities = Vec::new();
        let mut machines = Vec::new();
        for session in self.sessions() {
            let view = session.link.view();
            let Some(me) = view.me() else { continue };
            for account in &me.logins {
                let identity = HubAccount { hub: view.hub.clone(), account: account.clone() };
                if selected.is_some_and(|selected| selected != &identity)
                    || view.account(account).is_none_or(|account| account.locked)
                {
                    continue;
                }
                identities.push(identity);
                for machine in
                    view.machines().filter(|machine| machine.online && view.holds(account, &machine.name))
                {
                    if view.admits(&view.me, account, &machine.name, Purpose::Hands).is_ok() {
                        machines.push(Written { name: machine.name.clone(), hub: Some(session.link.hub_name()) });
                    }
                }
            }
        }
        identities.sort();
        identities.dedup();
        match identities.len() {
            0 => return Err(failed("this device is not logged in to the upload account")),
            1 => {}
            _ => return Err(failed("choose the account this upload belongs to")),
        }
        machines.sort();
        machines.dedup();
        let chosen = match preferred {
            Some(preferred) => Some(
                machines
                    .iter()
                    .find(|machine| {
                        machine.name == preferred.name
                            && preferred.hub.as_ref().is_none_or(|hub| Some(hub) == machine.hub.as_ref())
                    })
                    .cloned()
                    .ok_or_else(|| failed("choose one of this account's online machines for files"))?,
            ),
            None if machines.len() == 1 => Some(machines[0].clone()),
            None => None,
        };
        Ok(AttachmentPlaces { images_only: machines.is_empty(), machines, selected: chosen })
    }
}
