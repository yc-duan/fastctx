//! MCP execution output requires the original account AND current Work on the
//! executor. Qualifying a producer does not establish that this scope exists.
use crate::{Error, json, source::denied};
use fastctx_sync::{
    identity::{AccountId, DeviceId, OperationId, Producer},
    manifest::ContentRef,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpLog {
    pub account: AccountId,
    pub executor: DeviceId,
    pub execution: OperationId,
    pub command: String,
    pub content: ContentRef,
}
impl McpLog {
    pub fn validate(&self) -> Result<(), Error> {
        if self.command.is_empty() {
            return Err(json::invalid("MCP log requires its command identity"));
        }
        self.content.validate().map_err(Error::Invalid)
    }
}
/// A required audience, not a fabricated foundation scope or access capability.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MachineWorkAudience {
    pub account: AccountId,
    pub machine: DeviceId,
}
impl MachineWorkAudience {
    /// The adapter authenticates the reader and evaluates current Work using its
    /// own directory. A remembered grant or a valid old signature is insufficient.
    pub fn permits(&self, reader: &AccountId, current_work: bool) -> bool {
        reader == &self.account && current_work
    }
}
/// Taken from the real registered MCP command execution and its original account
/// incarnation, not the log's filename or the executor's current login.
pub fn authorize_source(value: &McpLog, producer: &Producer, executed: &McpLog) -> Result<MachineWorkAudience, Error> {
    value.validate()?;
    if producer != &(Producer::Device { device: value.executor.clone() }) || value != executed {
        return Err(denied("MCP log lacks its matching executor and original command execution"));
    }
    Ok(MachineWorkAudience { account: value.account.clone(), machine: value.executor.clone() })
}
