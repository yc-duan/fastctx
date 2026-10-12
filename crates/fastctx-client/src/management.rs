//! The user's management actions, identical for local and device clients.

use fastctx_types::management::{Managed, Management};
use fastctx_types::town::{MachineWho, WorldWho};
use fastctx_types::{Address, Call, MachineName, Reply, ResidentId, TownName};

use crate::{Error, World};

impl World {
    pub async fn manage(
        &self,
        resident: ResidentId,
        machine: Option<MachineName>,
        action: Management,
    ) -> Result<(Address, Managed), Error> {
        match self.ask(Call::Manage { resident, machine, action }).await? {
            Reply::Managed { resident, managed } => {
                if managed.removed.is_some() {
                    self.forget(vec![resident.clone()]).await?;
                }
                Ok((resident, managed))
            }
            reply => Err(Error::Unexpected(format!("{reply:?}"))),
        }
    }

    pub async fn remove(
        &self,
        resident: ResidentId,
        machine: Option<MachineName>,
    ) -> Result<(Address, Managed), Error> {
        self.manage(resident, machine, Management::Remove).await
    }

    pub async fn sleep(&self, resident: ResidentId, machine: Option<MachineName>) -> Result<(Address, Managed), Error> {
        self.manage(resident, machine, Management::Sleep).await
    }

    pub async fn wake(&self, resident: ResidentId, machine: Option<MachineName>) -> Result<(Address, Managed), Error> {
        self.manage(resident, machine, Management::Wake).await
    }

    pub async fn move_to_project(
        &self,
        resident: ResidentId,
        machine: Option<MachineName>,
        town: Option<TownName>,
    ) -> Result<(Address, Managed), Error> {
        self.manage(resident, machine, Management::Move { town }).await
    }

    /// Lets go of what this device keeps of the conversations `who` lists
    /// as removed, whichever device removed them. Machines that did not
    /// answer say nothing either way.
    pub(crate) async fn forget_removed(&self, who: &WorldWho) -> Result<(), Error> {
        let removed: Vec<Address> = who
            .machines
            .iter()
            .filter_map(|machine| match machine {
                MachineWho::Answered { reply } => Some(reply),
                MachineWho::Offline { .. } | MachineWho::NoAnswer { .. } => None,
            })
            .flat_map(|reply| {
                reply
                    .residents
                    .iter()
                    .filter(|resident| resident.removed.is_some())
                    .map(|resident| Address { machine: reply.machine.clone(), resident: resident.about.id.clone() })
            })
            .collect();
        self.forget(removed).await
    }

    /// A conversation removed or deleted is not kept here either
    /// (`world-gui.md`, 对话).
    async fn forget(&self, addresses: Vec<Address>) -> Result<(), Error> {
        let Some(cache) = self.cache().cloned() else { return Ok(()) };
        if addresses.is_empty() {
            return Ok(());
        }
        tokio::task::spawn_blocking(move || addresses.iter().try_for_each(|address| cache.forget(address)))
            .await
            .map_err(std::io::Error::other)
            .and_then(|forgot| forgot)
            .map_err(Error::Kept)
    }
}
