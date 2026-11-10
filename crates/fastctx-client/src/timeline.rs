//! A conversation's timeline, read a page at a time on its machine.

use crate::{Error, World};
use fastctx_types::{
    Call, Reply,
    access::Written,
    timeline_pages::{TimelineAsk, TimelineTold},
};

impl World {
    /// Reads a page, waits for a revision, or reads an object's range. An
    /// unmaterialized timeline answers a page with no revision or events.
    pub async fn timeline(&self, machine: Option<Written>, request: TimelineAsk) -> Result<TimelineTold, Error> {
        match self.ask(Call::Timeline { machine, request }).await? {
            Reply::Timeline(reply) => Ok(reply),
            other => Err(Error::Unexpected(format!("{other:?}"))),
        }
    }
}
