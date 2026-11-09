//! Connection facts belong to their reporting device and use current possession,
//! unlike the grant-segment visibility of management audit events.
use crate::{
    Error,
    source::{Binding, denied},
};
use fastctx_sync::{identity::DeviceId, store::ingest::SourceTarget};
use fastctx_types::{
    stats::LineChange,
    sync::scope::{ScopeDescriptor, ScopeSubject, ScopeVisibility},
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Connection {
    pub device: DeviceId,
    pub change: LineChange,
}

/// The scope description and source binding are authenticated service state.
/// Historical facts do not require the device to be online during replay.
pub fn authorize_source(
    value: &Connection,
    target: &SourceTarget,
    binding: &Binding,
    scope: &ScopeDescriptor,
) -> Result<(), Error> {
    binding.check(target, "machine.connection")?;
    binding.device(&value.device)?;
    if scope.scope != binding.object.scope
        || scope.acl_revision != binding.acl_revision
        || scope.subject != (ScopeSubject::Machine { device: value.device.clone() })
        || scope.visibility != ScopeVisibility::Current
    {
        return Err(denied("connection facts require the device's current-possession scope"));
    }
    Ok(())
}
