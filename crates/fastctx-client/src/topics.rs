//! Waiting for the node's state to change (`fastctx_types::topics`).

use crate::{Error, World};
use fastctx_types::{
    Call, Reply,
    topics::{TopicsAsk, TopicsTold},
};

impl World {
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
