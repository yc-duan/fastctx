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

    /// A piece is its part's bytes from an offset, the one object exactly
    /// as long as it says; the end refers to none. Once written, a piece is
    /// not written again otherwise.
    fn authorize(_actor: &Actor<'_>, before: Option<&Entry>, after: &KindRecord) -> Result<(), String> {
        let State::Is { body, objects } = &after.record.state else { return Ok(()) };
        let map = object(body)?;
        map.get("job").and_then(serde_json::Value::as_str).ok_or("a piece of output names its job")?;
        if map.get("end").and_then(serde_json::Value::as_bool) == Some(true) {
            return if objects.is_empty() {
                Ok(())
            } else {
                Err("the end of a job's output refers to no object".into())
            };
        }
        let part = map.get("part").and_then(serde_json::Value::as_str).unwrap_or_default();
        if !OUTPUT_PARTS.contains(&part) {
            return Err("a piece of output is of the output or the stderr".into());
        }
        let length =
            map.get("length").and_then(serde_json::Value::as_u64).ok_or("a piece of output says how long it is")?;
        match objects.as_slice() {
            [bytes] if bytes.size == length => {}
            _ => return Err("a piece of output is its one object of the length it says".into()),
        }
        if before.is_some_and(|before| before.body != *body || before.objects != *objects) {
            return Err("a piece of output, once written, stays as it is".into());
        }
        Ok(())
    }

    fn subscription(scope: &str, _stream: &str) -> String {
        format!("output:{scope}")
    }
}
