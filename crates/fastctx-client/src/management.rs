//! The user's management actions, identical for local and device clients.

use fastctx_types::management::{Managed, Management};
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
            Reply::Managed { resident, managed } => Ok((resident, managed)),
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
}
