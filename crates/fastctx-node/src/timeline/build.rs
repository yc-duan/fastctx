//! Making a conversation's timeline from its history as it is read whole:
//! the history is compared with the timeline by identity, and only what
//! changed is written. Reading the whole history costs what the history is
//! long; it is done here, after the fact and for all viewers at once, where
//! every viewer's opening once did it.

use super::identity::{self, Identity};
use super::order;
use crate::Node;
use fastctx_content::{Batch, Checkpoint, Content, RECORD_BYTES, Record, State, StreamKey};
use fastctx_types::ResidentId;
use fastctx_types::ipc::Failure;
use fastctx_types::session::SessionEvent;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// A body that is the event itself, as JSON after this byte.
pub(super) const EVENT: u8 = b'E';
/// A body that stands for an event kept whole as an object: after this byte,
/// its length and what is shown in its place.
pub(super) const OVERSIZED: u8 = b'O';

/// The records one batch brings to the timeline, at most.
const BATCH_RECORDS: usize = 2_048;
const BATCH_BYTES: usize = 4 * 1024 * 1024;

/// What a build came to.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Built {
    /// The events of the history read.
    pub events: usize,
    /// Those written, as new or in another place.
    pub written: usize,
    /// The items the history no longer holds.
    pub gone: usize,
}

fn failure(error: impl std::fmt::Display) -> Failure {
    Failure::new(error.to_string())
}

/// Reads the conversation's history, without keeping its host awake for the
/// reading, and brings its timeline to it. A history whose version is the
/// one the timeline was last built from is not read at all.
pub(crate) async fn build(node: &Arc<Node>, resident: &ResidentId) -> Result<Built, Failure> {
    let content = node.content()?.clone();
    let name = super::checkpoint(resident.as_str());
    // Taken before the history is read: what is read is at least as new.
    let version = crate::held_history::stamp(node, resident).await?.map(String::into_bytes);
    if let Some(version) = &version {
        let (reading, name) = (content.clone(), name.clone());
        let built = fastctx_platform::task::blocking(move || reading.checkpoint(&name)).await.map_err(failure)?;
        if built.as_ref() == Some(version) {
            return Ok(Built { events: 0, written: 0, gone: 0 });
        }
    }
    let crate::talk::HistoryRead { history, unread, .. } = crate::talk::history_quietly(node, resident).await?;
    let stream = super::stream(resident);
    // A reading that missed a record is not the version's history.
    let checkpoint = version.filter(|_| !unread).map(|state| Checkpoint { name, state });
    fastctx_platform::task::blocking(move || write(&content, &stream, &history, unread, checkpoint)).await
}

fn write(
    content: &Content,
    stream: &StreamKey,
    history: &[SessionEvent],
    unread: bool,
    checkpoint: Option<Checkpoint>,
) -> Result<Built, Failure> {
    let had: HashMap<Identity, u64> = content
        .places(stream)
        .map_err(failure)?
        .into_iter()
        .filter_map(|place| {
            let identity = place.item.as_slice().try_into().ok()?;
            Some((identity, u64::from_be_bytes(place.order.as_slice().try_into().ok()?)))
        })
        .collect();
    // A reading that missed some record is not the history as its files hold
    // it: what the timeline holds of it stays, until one reads it whole.
    if unread && !had.is_empty() {
        return Ok(Built { events: history.len(), written: 0, gone: 0 });
    }
    let encoded: Vec<Vec<u8>> = history.iter().map(serde_json::to_vec).collect::<Result<_, _>>().map_err(failure)?;
    let identities = identity::identities(history, &encoded);
    let kept: Vec<Option<u64>> = identities.iter().map(|identity| had.get(identity).copied()).collect();
    let keys = order::assign(&kept);
    let now: HashSet<&Identity> = identities.iter().collect();

    let mut pending = Pending { content, stream, records: Vec::new(), bytes: 0 };
    let mut gone = 0;
    for (identity, key) in &had {
        if !now.contains(identity) {
            pending.push(Record { item: identity.to_vec(), order: key.to_be_bytes().to_vec(), state: State::Gone })?;
            gone += 1;
        }
    }
    let mut written = 0;
    for (index, (event, encoded)) in history.iter().zip(encoded).enumerate() {
        if kept[index] == Some(keys[index]) {
            continue;
        }
        let state = state(content, event, encoded)?;
        pending.push(Record { item: identities[index].to_vec(), order: keys[index].to_be_bytes().to_vec(), state })?;
        written += 1;
    }
    pending.flush(checkpoint)?;
    Ok(Built { events: history.len(), written, gone })
}

/// An event as its record holds it: itself, or when it is too large for a
/// record, an object and what is shown in its place.
fn state(content: &Content, event: &SessionEvent, encoded: Vec<u8>) -> Result<State, Failure> {
    if encoded.len() < RECORD_BYTES {
        let mut body = Vec::with_capacity(encoded.len() + 1);
        body.push(EVENT);
        body.extend_from_slice(&encoded);
        return Ok(State::Is { body, objects: Vec::new() });
    }
    let object = content.objects().put(&encoded).map_err(failure)?;
    let mut body = vec![OVERSIZED];
    body.extend_from_slice(
        &serde_json::to_vec(&serde_json::json!({ "bytes": encoded.len(), "slim": super::preview::preview(event) }))
            .map_err(failure)?,
    );
    Ok(State::Is { body, objects: vec![object] })
}

/// Records on their way to the timeline, a bounded batch at a time. The
/// timeline is made again from its sources, so no batch waits for the disk.
struct Pending<'a> {
    content: &'a Content,
    stream: &'a StreamKey,
    records: Vec<Record>,
    bytes: usize,
}

impl Pending<'_> {
    fn push(&mut self, record: Record) -> Result<(), Failure> {
        self.bytes += 128
            + match &record.state {
                State::Is { body, .. } => body.len(),
                State::Gone => 0,
            };
        self.records.push(record);
        if self.records.len() >= BATCH_RECORDS || self.bytes >= BATCH_BYTES {
            self.flush(None)?;
        }
        Ok(())
    }

    /// Appends what is pending; with a checkpoint, as the build's last
    /// batch, which keeps the version the timeline now holds.
    fn flush(&mut self, checkpoint: Option<Checkpoint>) -> Result<(), Failure> {
        if self.records.is_empty() && checkpoint.is_none() {
            return Ok(());
        }
        let records = std::mem::take(&mut self.records);
        self.bytes = 0;
        self.content
            .append(Batch { stream: self.stream.clone(), generation: 0, records, checkpoint, durable: false })
            .map_err(failure)?;
        Ok(())
    }
}
