//! Trusted source coordinates supplied from committed provenance, never decoded
//! from a payload. Business checks supplement signatures and custody checks.
use crate::Error;
use fastctx_sync::{
    cursor::Revision,
    identity::{DeviceId, ObjectKey, Producer, ProducerEpoch, SourceId},
    store::ingest::SourceTarget,
};

/// The accepting service looks this up independently of the incoming target.
/// Local and hub acceptance use their own committed provenance and ACL snapshot.
#[derive(Clone, Debug)]
pub struct Binding {
    pub source: SourceId,
    pub object: ObjectKey,
    pub epoch: ProducerEpoch,
    pub producer: Producer,
    pub acl_revision: Revision,
}
impl Binding {
    pub fn check(&self, target: &SourceTarget, kind: &str) -> Result<(), Error> {
        if target.source != self.source
            || target.custody.object != self.object
            || target.epoch != self.epoch
            || target.producer != self.producer
            || target.custody.kind.registration().id != kind
            || target.custody.context.scope != self.object.scope
            || target.custody.context.observed_acl != self.acl_revision
        {
            return Err(denied("source does not match committed provenance"));
        }
        Ok(())
    }
    pub fn device(&self, device: &DeviceId) -> Result<(), Error> {
        if self.producer == (Producer::Device { device: device.clone() }) {
            Ok(())
        } else {
            Err(denied("a device may only report its own registered source"))
        }
    }
    pub fn hub(&self) -> Result<(), Error> {
        if self.producer == Producer::Hub { Ok(()) } else { Err(denied("only the hub may report this fact")) }
    }
}
pub(crate) fn denied(message: &str) -> Error {
    Error::Denied(message.into())
}
