//! What an account did on each machine, in its records' scope (design
//! §28.23, plan 3-4 group 7): each script `run` ran for it (`account.activity`,
//! in a stream `activity/<machine>` of the machine that ran it), one item a
//! deed as the machine's activity book has it, with the files it touched;
//! and what its agents spent (`account.usage`, in `usage/<machine>`), the
//! machine's usage lines of the account a piece at a time, each named by the
//! day's file and where in it the piece begins, its lines the one object.
//! Each machine orders its own streams; the hourly counts are made again
//! from the deeds where they are read.

use super::kind::{Actor, Deletion, Holding, Import, Kind};
use super::{Entry, KindRecord};
use crate::State;

fn object(body: &[u8]) -> Result<serde_json::Map<String, serde_json::Value>, String> {
    match serde_json::from_slice::<serde_json::Value>(body) {
        Ok(serde_json::Value::Object(map)) => Ok(map),
        Ok(_) => Err("an account's record is a JSON object".into()),
        Err(error) => Err(format!("an account's record is not JSON: {error}")),
    }
}

/// The deeds the machines ran for an account.
pub struct AccountActivity;

impl Kind for AccountActivity {
    /// `account.activity` in the registry, whose order only grows.
    const KIND: u16 = 25;
    const SCHEMA: u16 = 1;
    const STREAM: &'static str = "activity";
    const HOLDING: Holding = Holding::Readers;
    /// A deed larger than a body holds is its one object.
    const ROLES: &'static [&'static str] = &["deed"];
    const RETENTION: Option<u64> = None;
    const DELETION: Deletion = Deletion::Tombstone;
    /// Taken up from the machine's activity book.
    const IMPORT: Import = Import::From("activity-book");
    type Value = Vec<u8>;

    fn encode(value: &Vec<u8>) -> Vec<u8> {
        value.clone()
    }

    fn decode(body: &[u8]) -> Result<Vec<u8>, String> {
        object(body).map(|_| body.to_vec())
    }

    /// A deed is named by its key and refers to no object.
    fn authorize(_actor: &Actor<'_>, _before: Option<&Entry>, after: &KindRecord) -> Result<(), String> {
        let State::Is { body, objects } = &after.record.state else { return Ok(()) };
        let map = object(body)?;
        let in_object = map.get("in_object").and_then(serde_json::Value::as_bool) == Some(true);
        if objects.len() != usize::from(in_object) {
            return Err("a deed is its body, or the one object its body says it is".into());
        }
        let deed = map.get("deed").and_then(serde_json::Value::as_str).ok_or("a deed says its key")?;
        if deed.as_bytes() != after.record.item.as_slice() {
            return Err("a deed is named by its key".into());
        }
        Ok(())
    }

    fn subscription(scope: &str, stream: &str) -> String {
        format!("{stream}:{scope}")
    }
}

/// What the machines' agents spent for an account.
pub struct AccountUsage;

impl Kind for AccountUsage {
    /// `account.usage` in the registry, whose order only grows.
    const KIND: u16 = 26;
    const SCHEMA: u16 = 1;
    const STREAM: &'static str = "usage";
    const HOLDING: Holding = Holding::Readers;
    const ROLES: &'static [&'static str] = &["lines"];
    const RETENTION: Option<u64> = None;
    const DELETION: Deletion = Deletion::Tombstone;
    /// Taken up from the machine's usage ledgers.
    const IMPORT: Import = Import::From("usage");
    type Value = Vec<u8>;

    fn encode(value: &Vec<u8>) -> Vec<u8> {
        value.clone()
    }

    fn decode(body: &[u8]) -> Result<Vec<u8>, String> {
        object(body).map(|_| body.to_vec())
    }

    /// A piece is named by its day's file and where it begins, its one
    /// object as long as it says; once written, it stays as it is.
    fn authorize(_actor: &Actor<'_>, before: Option<&Entry>, after: &KindRecord) -> Result<(), String> {
        let State::Is { body, objects } = &after.record.state else {
            return Err("an account's usage is not let go".into());
        };
        if let Some(before) = before {
            return if before.body == *body && before.objects == *objects && before.order == after.record.order {
                Ok(())
            } else {
                Err("a piece of usage, once written, stays as it is".into())
            };
        }
        let map = object(body)?;
        let file = map.get("file").and_then(serde_json::Value::as_str).ok_or("a piece of usage names its file")?;
        let offset = map.get("offset").and_then(serde_json::Value::as_u64).ok_or("a piece of usage says where it begins")?;
        if after.record.item != format!("{file}/{offset:016x}").into_bytes() {
            return Err("a piece of usage is named by its file and where it begins".into());
        }
        let length = map.get("length").and_then(serde_json::Value::as_u64).ok_or("a piece of usage says how long it is")?;
        match objects.as_slice() {
            [lines] if lines.size == length => Ok(()),
            _ => Err("a piece of usage is its one object of the length it says".into()),
        }
    }

    fn subscription(scope: &str, stream: &str) -> String {
        format!("{stream}:{scope}")
    }
}
