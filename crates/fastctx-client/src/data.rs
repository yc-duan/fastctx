//! Data management is local to the computer making the request.

use fastctx_types::data::{DataCall, DataReply};
use fastctx_types::{Call, Reply};

use crate::world::{Error, World};

impl World {
    /// Lists or manages only the data kept by this computer's node.
    pub async fn data(&self, call: DataCall) -> Result<DataReply, Error> {
        if self.device().is_some() {
            return Err(Error::Other("this device has no local conversation data".into()));
        }
        match self.call(Call::Data(call)).await?.0 {
            Reply::Data(reply) => Ok(reply),
            other => Err(Error::Unexpected(format!("{other:?}"))),
        }
    }
}
