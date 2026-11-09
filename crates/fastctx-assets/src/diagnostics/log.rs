//! Append logs use file generations and committed byte ranges. Scene snapshots
//! are separate generations. The seven-day window schedules downloads, not expiry.
use crate::{Error, json, source::denied};
use fastctx_sync::{
    cursor::ByteCount,
    identity::{DeviceId, HostGeneration, Producer, SourceId},
    manifest::ContentRef,
};
use fastctx_types::sync::scope::{ScopeDescriptor, ScopeSubject, ScopeVisibility};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Origin {
    Machine { device: DeviceId },
    Hub,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Format {
    Lines,
    Scene,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Log {
    pub origin: Origin,
    pub source: SourceId,
    pub generation: HostGeneration,
    pub from: ByteCount,
    pub through: ByteCount,
    pub observed_at: Timestamp,
    pub format: Format,
    pub content: ContentRef,
}
impl Log {
    pub fn validate(&self) -> Result<(), Error> {
        self.content.validate().map_err(Error::Invalid)?;
        if self.through <= self.from || self.format == Format::Scene && self.from.0 != 0 {
            return Err(json::invalid("invalid committed log range"));
        }
        if let ContentRef::Single { decoded_bytes, .. } = self.content
            && u64::from(decoded_bytes) != self.through.0 - self.from.0
        {
            return Err(json::invalid("log range differs from its decoded content size"));
        }
        Ok(())
    }
}
/// Prepared from the actual local logger/file, with its content already stored
/// and its decoded length checked. No path or incoming body confers this role.
pub struct Evidence<'a> {
    pub producer: &'a Producer,
    pub committed: &'a Log,
    pub decoded_bytes: u64,
}
pub fn authorize_source(value: &Log, evidence: Evidence<'_>, scope: &ScopeDescriptor) -> Result<(), Error> {
    value.validate()?;
    let own = match (&value.origin, evidence.producer, &scope.subject) {
        (Origin::Machine { device }, Producer::Device { device: writer }, ScopeSubject::Machine { device: target }) => {
            device == writer && device == target && scope.visibility == ScopeVisibility::Current
        }
        (Origin::Hub, Producer::Hub, ScopeSubject::Administrators) => true,
        _ => false,
    };
    if !own
        || value != evidence.committed
        || value.through.0 - value.from.0 != evidence.decoded_bytes
        || value.content.id().domain != scope.scope
    {
        return Err(denied("log lacks its real producer, committed range or authorized content domain"));
    }
    Ok(())
}
/// Future-dated records remain eligible; old records stay available on demand.
pub fn download_recent(observed_at: Timestamp, now: Timestamp) -> bool {
    now.as_nanosecond() - observed_at.as_nanosecond() <= 7 * 86_400 * 1_000_000_000
}
