//! The activity book, and bringing a session the user opened on their own
//! into Fastctx (`world-layers.md`), through a node or directly from a
//! device's hub connection.

use crate::{Error, World};
use fastctx_types::access::Written;
use fastctx_types::activity_book::{BookAnswer, BookQuery};
use fastctx_types::{Call, Reply, ResidentId};

impl World {
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
