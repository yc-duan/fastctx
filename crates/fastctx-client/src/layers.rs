//! The account's layer (`world-layers.md`), through a node or directly from
//! a device's hub connection.

use crate::{Error, World};
use fastctx_types::access::Written;
use fastctx_types::activity_book::{BookAnswer, BookQuery};
use fastctx_types::layers::{Chosen, Layer};
use fastctx_types::{AccountName, Call, HubName, Reply, ResidentId};

impl World {
    /// The layer chosen, changed to `layer` when given.
    pub async fn layer(
        &self,
        hub: Option<HubName>,
        account: Option<AccountName>,
        layer: Option<Layer>,
    ) -> Result<Chosen, Error> {
        match self.ask(Call::Layer { hub, account, layer }).await? {
            Reply::Layer(chosen) => Ok(chosen),
            other => Err(Error::Unexpected(format!("{other:?}"))),
        }
    }

    /// The activity book of `machine`, or of this computer.
    pub async fn book(&self, machine: Option<Written>, query: BookQuery) -> Result<BookAnswer, Error> {
        match self.ask(Call::Book { machine, query }).await? {
            Reply::Book(answer) => Ok(*answer),
            other => Err(Error::Unexpected(format!("{other:?}"))),
        }
    }

    /// Brings a session of this machine the user opened on their own into
    /// Fastctx.
    pub async fn bring_in(&self, resident: ResidentId) -> Result<(), Error> {
        match self.ask(Call::BringIn { resident }).await? {
            Reply::Told => Ok(()),
            other => Err(Error::Unexpected(format!("{other:?}"))),
        }
    }
}
