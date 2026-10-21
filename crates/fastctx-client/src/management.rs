//! The user's management actions, identical for local and device clients.
//! An archived conversation keeps everything, the copy this device kept of
//! it included; a deleted one is forgotten on each machine and device as
//! its hub tells it (`fastctx_net::wire::Down::Forget`).

use fastctx_types::access::Written;
use fastctx_types::management::{Forgetting, Managed, Management};
use fastctx_types::{Address, Call, Reply, ResidentId, TownName};

use crate::{Error, World};

impl World {
    pub async fn manage(
        &self,
        resident: ResidentId,
        machine: Option<Written>,
        action: Management,
    ) -> Result<(Address, Managed), Error> {
        match self.ask(Call::Manage { resident, machine, action }).await? {
            Reply::Managed { resident, managed } => Ok((resident, managed)),
            reply => Err(Error::Unexpected(format!("{reply:?}"))),
        }
    }

    /// Takes the conversation out of the list into the archived ones; its
    /// turn, if it runs one, is stopped first.
    pub async fn archive(&self, resident: ResidentId, machine: Option<Written>) -> Result<(Address, Managed), Error> {
        self.manage(resident, machine, Management::Archive).await
    }

    /// Brings an archived conversation back where it was.
    pub async fn restore(&self, resident: ResidentId, machine: Option<Written>) -> Result<(Address, Managed), Error> {
        self.manage(resident, machine, Management::Restore).await
    }

    /// Deletes the conversation from every machine and device keeping
    /// something of it.
    pub async fn forget_conversation(&self, resident: ResidentId, machine: Written) -> Result<Forgetting, Error> {
        match self.ask(Call::Forget { resident, machine }).await? {
            Reply::Forgetting { how, .. } => Ok(how),
            reply => Err(Error::Unexpected(format!("{reply:?}"))),
        }
    }

    pub async fn sleep(&self, resident: ResidentId, machine: Option<Written>) -> Result<(Address, Managed), Error> {
        self.manage(resident, machine, Management::Sleep).await
    }

    pub async fn wake(&self, resident: ResidentId, machine: Option<Written>) -> Result<(Address, Managed), Error> {
        self.manage(resident, machine, Management::Wake).await
    }

    pub async fn move_to_project(
        &self,
        resident: ResidentId,
        machine: Option<Written>,
        town: Option<TownName>,
    ) -> Result<(Address, Managed), Error> {
        self.manage(resident, machine, Management::Move { town }).await
    }
}
