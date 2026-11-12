//! A conversation's timeline in the asset library (design §10, §11): made
//! from the history the content database's timeline is made from, and
//! written as the `Timeline` kind into the conversation's scope, its stream
//! ordered by this machine. Each event is one item named by what it says it
//! is — an item, words handed over, a turn's start or end — so an event that
//! changes is a new revision of the same item; an event that says no such
//! thing is named by its content. What is written is what changed since the
//! timeline here, item by item.
//!
//! The scope is the conversation's under the one account it keeps its
//! records with: its opener, and the machine it was born on when it was born
//! shared. Begun here, it is proposed to that account's hub, which describes
//! it at the driver's next round.

use super::{order, preview};
use crate::Node;
use crate::assets::Proposal;
use fastctx_content::v2::{EVENT, Entry, Kind as _, LEASE, Library, OVERSIZED, PAGE, Timeline, Write, Writer};
use fastctx_content::RECORD_BYTES;
use fastctx_types::access::HubId;
use fastctx_types::assets::{Holders, Readers, StreamClass};
use fastctx_types::ipc::Failure;
use fastctx_types::ResidentId;
use fastctx_types::residents::About;
use fastctx_types::session::SessionEvent;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Items one commit writes at most, and about how many bytes.
const WRITES_AT_ONCE: usize = 1_024;
const BYTES_AT_ONCE: usize = 4 * 1024 * 1024;

/// The conversations whose timeline is being written here now, and the
/// signal that one is done.
static WRITING: (std::sync::Mutex<std::collections::BTreeSet<String>>, std::sync::Condvar) =
    (std::sync::Mutex::new(std::collections::BTreeSet::new()), std::sync::Condvar::new());

/// One conversation's timeline written by one writer at a time: a build its
/// job gave up on, still writing, is done before the next one reads what is
/// here (AA08).
struct Writing(String);

impl Writing {
    fn take(resident: &ResidentId) -> Self {
        let (busy, done) = &WRITING;
        let mut busy = busy.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        while busy.contains(resident.as_str()) {
            busy = done.wait(busy).unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        busy.insert(resident.to_string());
        Self(resident.to_string())
    }
}

impl Drop for Writing {
    fn drop(&mut self) {
        let (busy, done) = &WRITING;
        busy.lock().unwrap_or_else(std::sync::PoisonError::into_inner).remove(&self.0);
        done.notify_all();
    }
}

fn failure(error: impl std::fmt::Display) -> Failure {
    Failure::new(error.to_string())
}

/// Where a conversation's timeline goes: its scope, and the hub it is
/// described by.
pub(super) struct Placed {
    pub(super) scope: String,
    pub(super) hub: HubId,
    pub(super) proposal: Proposal,
}

/// The name a conversation's producer keeps where it stands under.
pub(super) fn checkpoint(resident: &ResidentId) -> String {
    format!("timeline/{resident}")
}

/// Where the conversation's timeline goes, if it keeps its records with one
/// account: a conversation several accounts share alike has none yet.
pub(super) fn placed(node: &Node, about: &About) -> Result<Option<Placed>, Failure> {
    let Some(account) = crate::placement::account(node, &about.acting, None)? else { return Ok(None) };
    // The machine it was born on stays its place however it moves: a moved
    // conversation's timeline goes on there once its ordering is handed
    // over (design §8, plan 3-4), never in a scope of its own here (BB08).
    let born_on = crate::record_ownership::origin(node, about);
    let readers = Readers::Conversation { openers: vec![account.account.clone()], born_on };
    let scope = format!("{}{}", readers.key_prefix(account.hub.as_str()), about.id);
    let classes = vec![StreamClass { prefix: String::new(), holders: Holders::Readers }];
    Ok(Some(Placed { scope, hub: account.hub.clone(), proposal: Proposal { hub: account.hub.to_string(), readers, classes } }))
}

/// What the timeline here holds now, item by item.
fn held(library: &Library, scope: &str) -> Result<HashMap<Vec<u8>, Entry>, Failure> {
    let mut held = HashMap::new();
    let mut after = None;
    loop {
        let page = library.page(scope, Timeline::STREAM, after.clone(), PAGE).map_err(failure)?;
        let Some(last) = page.last() else { break };
        after = Some((last.order.clone(), last.item.clone()));
        let full = page.len() == PAGE;
        held.extend(page.into_iter().map(|entry| (entry.item.clone(), entry)));
        if !full {
            break;
        }
    }
    Ok(held)
}

/// What a write of a timeline left: how many items it wrote and how many
/// went, and the items of its window with their times and order keys.
pub(super) struct Wrote {
    pub(super) written: usize,
    pub(super) gone: usize,
    pub(super) items: Vec<(Vec<u8>, jiff::Timestamp, u64)>,
}

/// Where a timeline's window begins: the whole timeline, or the events from
/// `since` on, the items written of them before being `before` (identity
/// and time), every item before them ordered below `floor`.
pub(super) enum Span<'a> {
    Whole,
    From { since: jiff::Timestamp, before: &'a [(Vec<u8>, jiff::Timestamp)], floor: u64 },
}

/// Brings the conversation's timeline in the asset library to `history` over
/// `span`: the scope begun and proposed if it is not here yet; the window's
/// new and changed items written, those of the window gone from it removed.
/// An event of the history from before the window (the session's title,
/// the note of what could not be read) keeps the place it has. Call on a
/// thread that may block.
pub(super) fn write(
    library: &Library,
    signer: Arc<dyn fastctx_content::v2::Signer>,
    resident: &ResidentId,
    placed: &Placed,
    history: &[SessionEvent],
    span: Span<'_>,
    again: &HashSet<Vec<u8>>,
) -> Result<Wrote, Failure> {
    let _writing = Writing::take(resident);
    if library.holding(&placed.scope).map_err(failure)?.is_none() {
        let proposal = serde_json::to_vec(&placed.proposal).map_err(failure)?;
        library.propose(&placed.scope, proposal).map_err(failure)?;
    }
    let identities = super::names::identities(history);
    let (since, floor) = match &span {
        Span::Whole => (None, None),
        Span::From { since, floor, .. } => (Some(*since), Some(*floor)),
    };
    let had: HashMap<Vec<u8>, Entry> = match &span {
        Span::Whole => held(library, &placed.scope)?,
        Span::From { before, .. } => {
            let mut had = HashMap::new();
            for identity in identities.iter().chain(before.iter().map(|(identity, _)| identity)) {
                if !had.contains_key(identity)
                    && let Some(entry) = library.item(&placed.scope, Timeline::STREAM, identity).map_err(failure)?
                {
                    had.insert(identity.clone(), entry);
                }
            }
            had
        }
    };
    let key_of = |identity: &Vec<u8>| had.get(identity).and_then(|entry| entry.order.as_slice().try_into().ok().map(u64::from_be_bytes));
    // The window's events take keys past every item before it; one of its
    // own below that is placed again.
    // An event from before the window keeps the place it has; one new to
    // the timeline, though its time is earlier, takes its place in the
    // window, never the very first (CC14); and so does one told again at
    // every reading where the whole history has it (`again`, N1).
    let early: Vec<bool> = history
        .iter()
        .zip(&identities)
        .map(|(event, identity)| since.is_some_and(|since| event.at < since) && had.contains_key(identity) && !again.contains(identity))
        .collect();
    let window: Vec<usize> = (0..history.len()).filter(|index| !early[*index]).collect();
    let mut keyed: Vec<Option<u64>> = Vec::with_capacity(window.len() + 1);
    if let Some(floor) = floor {
        keyed.push(Some(floor));
    }
    keyed.extend(window.iter().map(|&index| key_of(&identities[index]).filter(|key| floor.is_none_or(|floor| *key > floor))));
    let assigned = order::assign(&keyed);
    let assigned = if floor.is_some() { &assigned[1..] } else { &assigned[..] };
    let mut keys = vec![0u64; history.len()];
    for (&index, key) in window.iter().zip(assigned) {
        keys[index] = *key;
    }
    for index in (0..history.len()).filter(|index| early[*index]) {
        keys[index] = key_of(&identities[index]).unwrap_or(0);
    }
    let present: HashSet<&Vec<u8>> = identities.iter().collect();
    let gone_items: Vec<(&Vec<u8>, &Entry)> = match &span {
        Span::Whole => had.iter().filter(|(identity, _)| !present.contains(identity)).collect(),
        Span::From { since, before, .. } => before
            .iter()
            .filter(|(identity, at)| at >= since && !present.contains(identity))
            .filter_map(|(identity, _)| had.get_key_value(identity))
            .collect(),
    };
    let writer = Writer::<Timeline>::new(library, &placed.scope, Timeline::STREAM).map_err(failure)?;
    let mut pending = Pending { writer: &writer, signer, writes: Vec::new(), leases: Vec::new(), bytes: 0 };
    let gone = gone_items.len();
    for (identity, entry) in gone_items {
        pending.push(Write { item: identity.clone(), order: entry.order.clone(), value: None, objects: Vec::new() }, None, 0)?;
    }
    let mut written = 0;
    for (index, event) in history.iter().enumerate() {
        let encoded = serde_json::to_vec(event).map_err(failure)?;
        let order = keys[index].to_be_bytes().to_vec();
        let (body, object) = body(event, &encoded)?;
        // Compared before anything is put: an unchanged large event writes
        // nothing again (AA11).
        let objects: Vec<fastctx_content::ObjectRef> = object.into_iter().collect();
        if let Some(entry) = had.get(&identities[index])
            && entry.order == order
            && entry.body == body
            && entry.objects == objects
        {
            continue;
        }
        let lease = match objects.first() {
            Some(object) => {
                let lease = library.put(&mut encoded.as_slice(), &format!("timeline:{resident}"), LEASE).map_err(failure)?;
                if lease.object != *object {
                    return Err(failure("an event's object is not what it was taken for"));
                }
                Some(lease)
            }
            None => None,
        };
        let bytes = body.len();
        pending.push(Write { item: identities[index].clone(), order, value: Some(body), objects }, lease, bytes)?;
        written += 1;
    }
    pending.flush()?;
    let items = window.iter().map(|&index| (identities[index].clone(), history[index].at, keys[index])).collect();
    Ok(Wrote { written, gone, items })
}

/// An event as its record holds it: itself, or when it is too large for a
/// record, what is shown in its place, with the object the whole of it is
/// (put only when the record is written).
fn body(event: &SessionEvent, encoded: &[u8]) -> Result<(Vec<u8>, Option<fastctx_content::ObjectRef>), Failure> {
    if encoded.len() < RECORD_BYTES {
        let mut body = Vec::with_capacity(encoded.len() + 1);
        body.push(EVENT);
        body.extend_from_slice(encoded);
        return Ok((body, None));
    }
    let object = fastctx_content::ObjectRef { hash: Sha256::digest(encoded).into(), size: encoded.len() as u64 };
    let mut body = vec![OVERSIZED];
    body.extend_from_slice(
        &serde_json::to_vec(&serde_json::json!({
            "bytes": encoded.len(),
            "slim": preview::preview(event),
            "likeness": super::identity::likeness(event, encoded),
        }))
        .map_err(failure)?,
    );
    Ok((body, Some(object)))
}

/// Writes on their way to the timeline, a bounded commit at a time. The
/// timeline is made again from its sources, so no commit waits for the disk.
struct Pending<'a> {
    writer: &'a Writer<Timeline>,
    signer: Arc<dyn fastctx_content::v2::Signer>,
    writes: Vec<Write<Timeline>>,
    leases: Vec<fastctx_content::v2::Lease>,
    bytes: usize,
}

impl Pending<'_> {
    fn push(&mut self, write: Write<Timeline>, lease: Option<fastctx_content::v2::Lease>, bytes: usize) -> Result<(), Failure> {
        self.bytes += 128 + bytes;
        self.writes.push(write);
        self.leases.extend(lease);
        if self.writes.len() >= WRITES_AT_ONCE || self.bytes >= BYTES_AT_ONCE {
            self.flush()?;
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<(), Failure> {
        if self.writes.is_empty() {
            return Ok(());
        }
        let (writes, leases) = (std::mem::take(&mut self.writes), std::mem::take(&mut self.leases));
        self.bytes = 0;
        self.writer.write_all(writes, leases, false, self.signer.clone()).map_err(failure)?;
        Ok(())
    }
}
