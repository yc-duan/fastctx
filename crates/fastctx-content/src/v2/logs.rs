//! Logs in the asset library (design §14, §28.21): a machine's own log
//! (`machine.log`, stream `node`, in its body's scope) and the hub's
//! (`hub.log`, stream `hub`, in the hub's scope) as pieces of whole lines,
//! each named by its file and where it begins and written once; and the
//! logs of the MCP commands an account runs on a machine (`mcp.log`, stream
//! `mcp`, in that account's scope on that machine), each as it stands now.
//! The hub keeps every piece; a device keeps the latest `KEPT_PIECES` of a
//! log's pieces, about a week's at a piece a minute.

use super::kind::{Actor, Deletion, Holding, Import, Kind};
use super::{Entry, KindRecord};
use crate::State;

/// The pieces of a log a device keeps: a week of minutes.
pub const KEPT_PIECES: u64 = 7 * 24 * 60;

fn object(body: &[u8]) -> Result<serde_json::Map<String, serde_json::Value>, String> {
    match serde_json::from_slice::<serde_json::Value>(body) {
        Ok(serde_json::Value::Object(map)) => Ok(map),
        Ok(_) => Err("a log's record is a JSON object".into()),
        Err(error) => Err(format!("a log's record is not JSON: {error}")),
    }
}

/// A piece is its bytes from where it begins in its file, named by both, the
/// one object exactly as long as it says; once written, it stays as it is.
fn piece(before: Option<&Entry>, after: &KindRecord) -> Result<(), String> {
    let State::Is { body, objects } = &after.record.state else {
        return Err("a piece of a log is not let go: a device lets the oldest frames go".into());
    };
    if let Some(before) = before {
        return if before.body == *body && before.objects == *objects {
            Ok(())
        } else {
            Err("a piece of a log, once written, stays as it is".into())
        };
    }
    let map = object(body)?;
    let file = map.get("file").and_then(serde_json::Value::as_str).ok_or("a piece of a log names its file")?;
    let offset = map.get("offset").and_then(serde_json::Value::as_u64).ok_or("a piece of a log says where it begins")?;
    if after.record.item != format!("{file}/{offset:016x}").into_bytes() {
        return Err("a piece of a log is named by its file and where it begins".into());
    }
    let length = map.get("length").and_then(serde_json::Value::as_u64).ok_or("a piece of a log says how long it is")?;
    match objects.as_slice() {
        [bytes] if bytes.size == length => Ok(()),
        _ => Err("a piece of a log is its one object of the length it says".into()),
    }
}

macro_rules! log_pieces {
    ($(#[$doc:meta])* $name:ident, $number:literal, $stream:literal, $import:literal) => {
        $(#[$doc])*
        pub struct $name;

        impl Kind for $name {
            const KIND: u16 = $number;
            const SCHEMA: u16 = 1;
            const STREAM: &'static str = $stream;
            const HOLDING: Holding = Holding::Readers;
            const ROLES: &'static [&'static str] = &["bytes"];
            const RETENTION: Option<u64> = Some(KEPT_PIECES);
            const DELETION: Deletion = Deletion::Tombstone;
            /// Taken up from the log file as it grows.
            const IMPORT: Import = Import::From($import);
            type Value = Vec<u8>;

            fn encode(value: &Vec<u8>) -> Vec<u8> {
                value.clone()
            }

            fn decode(body: &[u8]) -> Result<Vec<u8>, String> {
                object(body).map(|_| body.to_vec())
            }

            fn authorize(_actor: &Actor<'_>, before: Option<&Entry>, after: &KindRecord) -> Result<(), String> {
                piece(before, after)
            }

            fn subscription(scope: &str, _stream: &str) -> String {
                format!("{}:{scope}", $stream)
            }
        }
    };
}

log_pieces!(
    /// A machine's log (`machine.log`), written by the machine.
    MachineLog, 53, "node", "node-log"
);
log_pieces!(
    /// The hub's log (`hub.log`), written by the hub.
    HubLog, 55, "hub", "hub-log"
);

/// The log of one MCP command an account runs on a machine (`mcp.log`), by
/// the command's name: the one object is the log as it stands, as long as
/// the record says; a command gone takes its log with it.
pub struct McpLog;

impl Kind for McpLog {
    const KIND: u16 = 54;
    const SCHEMA: u16 = 1;
    const STREAM: &'static str = "mcp";
    const HOLDING: Holding = Holding::Readers;
    const ROLES: &'static [&'static str] = &["bytes"];
    const RETENTION: Option<u64> = None;
    const DELETION: Deletion = Deletion::Tombstone;
    /// Taken up from the command's log file.
    const IMPORT: Import = Import::From("mcp-log");
    type Value = Vec<u8>;

    fn encode(value: &Vec<u8>) -> Vec<u8> {
        value.clone()
    }

    fn decode(body: &[u8]) -> Result<Vec<u8>, String> {
        object(body).map(|_| body.to_vec())
    }

    fn authorize(_actor: &Actor<'_>, _before: Option<&Entry>, after: &KindRecord) -> Result<(), String> {
        let State::Is { body, objects } = &after.record.state else { return Ok(()) };
        let map = object(body)?;
        let name = map.get("name").and_then(serde_json::Value::as_str).ok_or("an MCP command's log names it")?;
        if after.record.item != name.as_bytes() || name.is_empty() || name.contains('/') {
            return Err("an MCP command's log is named by the command".into());
        }
        let length = map.get("length").and_then(serde_json::Value::as_u64).ok_or("an MCP command's log says how long it is")?;
        match objects.as_slice() {
            [bytes] if bytes.size == length => Ok(()),
            _ => Err("an MCP command's log is its one object of the length it says".into()),
        }
    }

    fn subscription(scope: &str, _stream: &str) -> String {
        format!("mcp:{scope}")
    }
}
