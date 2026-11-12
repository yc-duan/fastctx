//! Kinds for the drills alone.

use fastctx_content::v2::{Actor, Deletion, Entry, Holding, Import, Kind, KindRecord};

/// A kind anyone may write, for the drills.
pub(crate) struct Plain;

impl Kind for Plain {
    const KIND: u16 = 1;
    const SCHEMA: u16 = 1;
    const STREAM: &'static str = "";
    const HOLDING: Holding = Holding::Readers;
    const ROLES: &'static [&'static str] = &["first", "second", "third", "fourth"];
    const RETENTION: Option<u64> = None;
    const DELETION: Deletion = Deletion::Tombstone;
    const IMPORT: Import = Import::Nothing;
    type Value = Vec<u8>;
    fn encode(value: &Vec<u8>) -> Vec<u8> {
        value.clone()
    }
    fn decode(body: &[u8]) -> Result<Vec<u8>, String> {
        Ok(body.to_vec())
    }
    fn authorize(_: &Actor<'_>, _: Option<&Entry>, _: &KindRecord) -> Result<(), String> {
        Ok(())
    }
    fn subscription(_: &str, _: &str) -> String {
        "plain".into()
    }
}
