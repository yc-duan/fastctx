//! Waiting for the node's state to change (`fastctx_types::topics`).

use crate::{Error, World};
use fastctx_types::{
    Call, Reply,
    topics::{TopicsAsk, TopicsTold},
};

impl World {
    /// Waits on topics when this local node advertised them. Devices and
    /// older nodes return none so a reader can keep its timed reads.
    pub async fn topics_when_supported(&self, request: TopicsAsk) -> Result<Option<TopicsTold>, Error> {
        if self.device().is_some() {
            return Ok(None);
        }
        let mut client = self.connect().await?;
        if !client.topics {
            return Ok(None);
        }
        match client.call(Call::Topics { request }).await? {
            Reply::Topics(told) => Ok(Some(told)),
            other => Err(Error::Unexpected(format!("{other:?}"))),
        }
    }

    /// Asks where topics stand, or waits until one is past where the reader
    /// read it. A node that tells no topics refuses, and the reader reads
    /// on a timer as before.
    pub async fn topics(&self, request: TopicsAsk) -> Result<TopicsTold, Error> {
        match self.ask(Call::Topics { request }).await? {
            Reply::Topics(told) => Ok(told),
            other => Err(Error::Unexpected(format!("{other:?}"))),
        }
    }
}
