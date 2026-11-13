//! A conversation's sentinels' wakes, as the machine each lives on keeps
//! them (design §29 2.7, registry 9 `conversation.sentinel-run`): each wake
//! a sentinel committed, in the stream of the machine it lives on,
//! `sentinel-runs/<that machine's key's first 8 bytes>`, named by the
//! sentinel, its generation and the wake's sequence. The body says which
//! wake and how it ended; its objects are the state the wake left and,
//! where the machine kept it, the wake's run. Written by the machine the
//! stream names alone; a wake once written stays as it is.

use super::kind::{Actor, Deletion, Holding, Import, Kind};
use super::{Entry, KindRecord};
use crate::State;

/// The wakes of a conversation's sentinels.
pub struct ConversationSentinelRun;

/// The body of a wake as an object, with its name.
fn named(body: &[u8]) -> Result<String, String> {
    let value: serde_json::Value = serde_json::from_slice(body).map_err(|error| format!("a sentinel's wake: {error}"))?;
    let sentinel = value.get("sentinel").and_then(serde_json::Value::as_str).ok_or("a sentinel's wake names its sentinel")?;
    let generation = value.get("generation").and_then(serde_json::Value::as_u64).ok_or("a sentinel's wake says its generation")?;
    let sequence = value.get("sequence").and_then(serde_json::Value::as_u64).ok_or("a sentinel's wake says its sequence")?;
    Ok(format!("{sentinel}/{generation}/{sequence:020}"))
}

impl Kind for ConversationSentinelRun {
    /// `conversation.sentinel-run` in the registry, whose order only grows.
    const KIND: u16 = 9;
    const SCHEMA: u16 = 1;
    const STREAM: &'static str = "sentinel-runs";
    const HOLDING: Holding = Holding::Readers;
    const ROLES: &'static [&'static str] = &["state", "run"];
    const RETENTION: Option<u64> = None;
    const DELETION: Deletion = Deletion::Tombstone;
    /// Made again from the sentinels' own files on the machine they live on.
    const IMPORT: Import = Import::Remade;
    type Value = Vec<u8>;

    fn encode(value: &Vec<u8>) -> Vec<u8> {
        value.clone()
    }

    fn decode(body: &[u8]) -> Result<Vec<u8>, String> {
        named(body)?;
        Ok(body.to_vec())
    }

    /// Only the machine its stream names writes it; a wake is named by its
    /// sentinel, generation and sequence, has the state it left as its first
    /// object, and once written stays as it is.
    fn authorize(actor: &Actor<'_>, before: Option<&Entry>, after: &KindRecord) -> Result<(), String> {
        let hex: String = actor.device[..8].iter().map(|byte| format!("{byte:02x}")).collect();
        if actor.stream.strip_prefix("sentinel-runs/") != Some(hex.as_str()) {
            return Err("a sentinel's wakes are written by the machine their stream names".into());
        }
        let State::Is { body, objects } = &after.record.state else {
            return Ok(());
        };
        if let Some(before) = before {
            return if before.body == *body && before.objects == *objects && before.order == after.record.order {
                Ok(())
            } else {
                Err("a sentinel's wake, once written, stays as it is".into())
            };
        }
        if super::names::item_of(&named(body)?) != after.record.item {
            return Err("a sentinel's wake is named by its sentinel, generation and sequence".into());
        }
        if objects.is_empty() {
            return Err("a sentinel's wake holds the state it left".into());
        }
        Ok(())
    }

    fn subscription(scope: &str, stream: &str) -> String {
        format!("{stream}:{scope}")
    }
}
