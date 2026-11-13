//! The kinds of a job's facts and output (design §13, plan 3-4 group 3), in
//! the scope of the conversation that started it. Its facts are one item, the
//! job's record as it stands, made again as it changes. Its output only grows:
//! each piece of bytes that came since is an item of its own, named by the
//! part and the offset it begins at, its bytes an object, written once and
//! never changed; when the job ended and every byte is written, one more
//! item says so, with each part's length. Readers put the pieces together by
//! offset; nothing republishes the whole output. Written by the machine the
//! job ran on, which orders the streams.

use super::kind::{Actor, Deletion, Holding, Import, Kind};
use super::{Entry, KindRecord};
use crate::State;

/// The parts of a job's output.
pub const OUTPUT_PARTS: [&str; 2] = ["output", "stderr"];

/// A job's facts.
pub struct JobFacts;

/// A job's output, piece by piece.
pub struct JobOutput;

fn object(body: &[u8]) -> Result<serde_json::Map<String, serde_json::Value>, String> {
    match serde_json::from_slice::<serde_json::Value>(body) {
        Ok(serde_json::Value::Object(map)) => Ok(map),
        Ok(_) => Err("a job's record is one JSON object".into()),
        Err(error) => Err(format!("a job's record is not JSON: {error}")),
    }
}

impl Kind for JobFacts {
    /// `job.facts` in the registry (`fastctx_sync::registry`), whose order
    /// only grows.
    const KIND: u16 = 20;
    const SCHEMA: u16 = 1;
    const STREAM: &'static str = "jobs";
    const HOLDING: Holding = Holding::Readers;
    const ROLES: &'static [&'static str] = &[];
    const RETENTION: Option<u64> = None;
    const DELETION: Deletion = Deletion::Tombstone;
    /// Made again from the job's record.
    const IMPORT: Import = Import::Remade;
    type Value = Vec<u8>;

    fn encode(value: &Vec<u8>) -> Vec<u8> {
        value.clone()
    }

    fn decode(body: &[u8]) -> Result<Vec<u8>, String> {
        object(body)?.get("id").and_then(serde_json::Value::as_str).ok_or("a job's facts name it")?;
        Ok(body.to_vec())
    }

    fn authorize(_actor: &Actor<'_>, _before: Option<&Entry>, after: &KindRecord) -> Result<(), String> {
        match &after.record.state {
            State::Gone => Ok(()),
            State::Is { body, objects } if objects.is_empty() => Self::decode(body).map(|_| ()),
            State::Is { .. } => Err("a job's facts refer to no object".into()),
        }
    }

    fn subscription(scope: &str, _stream: &str) -> String {
        format!("jobs:{scope}")
    }
}

impl Kind for JobOutput {
    /// `job.output` in the registry.
    const KIND: u16 = 21;
    const SCHEMA: u16 = 1;
    const STREAM: &'static str = "output";
    const HOLDING: Holding = Holding::Readers;
    const ROLES: &'static [&'static str] = &["bytes"];
    const RETENTION: Option<u64> = None;
    const DELETION: Deletion = Deletion::Tombstone;
    /// Taken up from the job's log as it grows.
    const IMPORT: Import = Import::From("job-output");
    type Value = Vec<u8>;

    fn encode(value: &Vec<u8>) -> Vec<u8> {
        value.clone()
    }

    fn decode(body: &[u8]) -> Result<Vec<u8>, String> {
        let map = object(body)?;
        map.get("job").and_then(serde_json::Value::as_str).ok_or("a piece of output names its job")?;
        Ok(body.to_vec())
    }

    /// A piece is its part's bytes from an offset, named by its job, part
    /// and offset, the one object exactly as long as it says; the end, named
    /// by its job, says each part's length and refers to none. Once written,
    /// neither is written again otherwise, nor let go (AQ09, AQ12).
    fn authorize(_actor: &Actor<'_>, before: Option<&Entry>, after: &KindRecord) -> Result<(), String> {
        let State::Is { body, objects } = &after.record.state else {
            return Err("a job's output is not let go".into());
        };
        if let Some(before) = before {
            return if before.body == *body && before.objects == *objects && before.order == after.record.order {
                Ok(())
            } else {
                Err("a piece of output, once written, stays as it is".into())
            };
        }
        let map = object(body)?;
        let job = map.get("job").and_then(serde_json::Value::as_str).ok_or("a piece of output names its job")?;
        let item = String::from_utf8_lossy(&after.record.item);
        if map.get("end").and_then(serde_json::Value::as_bool) == Some(true) {
            let lengths = OUTPUT_PARTS.iter().all(|part| map.get(*part).and_then(serde_json::Value::as_u64).is_some());
            return if !objects.is_empty() {
                Err("the end of a job's output refers to no object".into())
            } else if !lengths || item != format!("{job}/end") {
                Err("the end of a job's output names its job and says each part's length".into())
            } else {
                Ok(())
            };
        }
        let part = map.get("part").and_then(serde_json::Value::as_str).unwrap_or_default();
        if !OUTPUT_PARTS.contains(&part) {
            return Err("a piece of output is of the output or the stderr".into());
        }
        let offset =
            map.get("offset").and_then(serde_json::Value::as_u64).ok_or("a piece of output says where it begins")?;
        if item != format!("{job}/{part}/{offset:016x}") {
            return Err("a piece of output is named by its job, part and offset".into());
        }
        let length =
            map.get("length").and_then(serde_json::Value::as_u64).ok_or("a piece of output says how long it is")?;
        match objects.as_slice() {
            [bytes] if bytes.size == length => Ok(()),
            _ => Err("a piece of output is its one object of the length it says".into()),
        }
    }

    fn subscription(scope: &str, _stream: &str) -> String {
        format!("output:{scope}")
    }
}
