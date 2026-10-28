//! The collaboration tree through a local node or a device's authenticated route.

use crate::{Error, World};
use fastctx_types::{Call, Reply, ResidentId, access::Written, collaboration::Tree};

impl World {
    pub async fn collaboration(&self, resident: ResidentId, machine: Option<Written>) -> Result<Tree, Error> {
        match self.ask(Call::Collaboration { resident, machine }).await? {
            Reply::Collaboration(tree) => Ok(*tree),
            other => Err(Error::Unexpected(format!("{other:?}"))),
        }
    }
}
