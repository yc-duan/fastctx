//! Model collaboration, through a local node or a device connection: an
//! account's, or, with no account named, the computer's own, which applies
//! while it is in no network.

use fastctx_types::collab::{Collab, CollabView};
use fastctx_types::{AccountName, Call, HubName, Reply};

use crate::{Error, World};

impl World {
    pub async fn collab_read(&self, hub: Option<HubName>, account: Option<AccountName>) -> Result<CollabView, Error> {
        match self.ask(Call::CollabRead { hub, account }).await? {
            Reply::CollabRead(view) => Ok(view),
            other => Err(Error::Unexpected(format!("{other:?}"))),
        }
    }

    pub async fn collab_write(
        &self,
        hub: Option<HubName>,
        account: Option<AccountName>,
        collab: Collab,
    ) -> Result<u64, Error> {
        match self.ask(Call::CollabWrite { hub, account, collab }).await? {
            Reply::CollabWritten { version } => Ok(version),
            other => Err(Error::Unexpected(format!("{other:?}"))),
        }
    }
}
