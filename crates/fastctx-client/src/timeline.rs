//! A conversation's installed timeline, read through the local node.

use crate::{Error, World};
use fastctx_types::{
    Call, Reply,
    timeline_reads::{TimelineCall, TimelineReply},
};

impl World {
    /// `Unavailable` means the conversation has no timeline installed on its
    /// machine yet; its history is read as before.
    pub async fn timeline(&self, call: TimelineCall) -> Result<TimelineReply, Error> {
        match self.ask(Call::Timeline(call)).await? {
            Reply::Timeline(reply) => Ok(reply),
            other => Err(Error::Unexpected(format!("{other:?}"))),
        }
    }
}
