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
fn checkpoint(resident: &ResidentId) -> String {
    format!("timeline/{resident}")
}

/// Where the conversation's timeline goes, if it keeps its records with one
/// account: a conversation several accounts share alike has none yet.
pub(super) fn placed(node: &Node, about: &About) -> Result<Option<Placed>, Failure> {
    let Some(account) = crate::placement::account(node, &about.acting, None)? else { return Ok(None) };
    let born_on = crate::record_ownership::origin(node, about);
    let readers = Readers::Conversation { openers: vec![account.account.clone()], born_on };
    let scope = format!("{}{}", readers.key_prefix(), about.id);
    let classes = vec![StreamClass { prefix: String::new(), holders: Holders::Readers }];
    Ok(Some(Placed { scope, hub: account.hub.clone(), proposal: Proposal { hub: account.hub.to_string(), readers, classes } }))
}

/// Whether the timeline in the asset library lags `version` of the history.
pub(super) fn behind(library: &Library, resident: &ResidentId, version: Option<&[u8]>) -> Result<bool, Failure> {
    let kept = library.checkpoint(&checkpoint(resident)).map_err(failure)?;
    Ok(version.is_none() || kept.as_deref() != version)
}

/// What names each event of `history`, given each as it is encoded: what it
/// says it is, else what it holds; events of one name told apart by their
/// order among themselves.
fn identities(history: &[SessionEvent], encoded: &[Vec<u8>]) -> Vec<Vec<u8>> {
    let mut seen: HashMap<[u8; 32], u32> = HashMap::new();
    history
        .iter()
        .zip(encoded)
        .map(|(event, encoded)| {
            let mut hasher = Sha256::new();
            hasher.update(b"fastctx-timeline/1");
            match super::identity::name(event) {
                Some(name) => {
                    hasher.update(b"named");
                    hasher.update(name.as_bytes());
                }
                None => {
                    hasher.update(b"content");
                    hasher.update(Sha256::digest(encoded));
                }
            }
            let named: [u8; 32] = hasher.finalize().into();
            let before = seen.entry(named).or_insert(0);
            let identity = if *before == 0 {
                named
            } else {
                let mut again = Sha256::new();
                again.update(named);
                again.update(before.to_le_bytes());
                again.finalize().into()
            };
            *before += 1;
            identity.to_vec()
        })
        .collect()
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

/// Brings the conversation's timeline in the asset library to `history`, at
/// `version`: the scope begun and proposed if it is not here yet. How many
/// items were written and how many went. Call on a thread that may block.
pub(super) fn write(
    library: &Library,
    signer: Arc<dyn fastctx_content::v2::Signer>,
    resident: &ResidentId,
    placed: &Placed,
    history: &[SessionEvent],
    version: Option<Vec<u8>>,
) -> Result<(usize, usize), Failure> {
    if library.holding(&placed.scope).map_err(failure)?.is_none() {
        let proposal = serde_json::to_vec(&placed.proposal).map_err(failure)?;
        library.propose(&placed.scope, proposal).map_err(failure)?;
    }
    let had = held(library, &placed.scope)?;
    let encoded: Vec<Vec<u8>> = history.iter().map(serde_json::to_vec).collect::<Result<_, _>>().map_err(failure)?;
    let identities = identities(history, &encoded);
    let kept: Vec<Option<u64>> = identities
        .iter()
        .map(|identity| had.get(identity).and_then(|entry| entry.order.as_slice().try_into().ok().map(u64::from_be_bytes)))
        .collect();
    let keys = order::assign(&kept);
    let now: HashSet<&Vec<u8>> = identities.iter().collect();
    let writer = Writer::<Timeline>::new(library, &placed.scope, Timeline::STREAM).map_err(failure)?;
    let mut pending = Pending { writer: &writer, signer, writes: Vec::new(), leases: Vec::new(), bytes: 0 };
    let mut gone = 0;
    for (identity, entry) in &had {
        if !now.contains(identity) {
            pending.push(Write { item: identity.clone(), order: entry.order.clone(), value: None, objects: Vec::new() }, None, 0)?;
            gone += 1;
        }
    }
    let mut written = 0;
    for (index, (event, encoded)) in history.iter().zip(encoded).enumerate() {
        let order = keys[index].to_be_bytes().to_vec();
        let (body, object) = body(library, resident, event, encoded)?;
        if let Some(entry) = had.get(&identities[index])
            && entry.order == order
            && entry.body == body
            && entry.objects == object.as_ref().map(|lease| vec![lease.object]).unwrap_or_default()
        {
            if let Some(lease) = object {
                library.release(lease).map_err(failure)?;
            }
            continue;
        }
        let objects = object.as_ref().map(|lease| vec![lease.object]).unwrap_or_default();
        let bytes = body.len();
        pending.push(Write { item: identities[index].clone(), order, value: Some(body), objects }, object, bytes)?;
        written += 1;
    }
    pending.flush()?;
    if let Some(version) = version {
        library.keep_checkpoint(&checkpoint(resident), version).map_err(failure)?;
    }
    Ok((written, gone))
}

/// An event as its record holds it: itself, or when it is too large for a
/// record, what is shown in its place, the whole of it an object leased
/// until the record refers to it.
fn body(library: &Library, resident: &ResidentId, event: &SessionEvent, encoded: Vec<u8>) -> Result<(Vec<u8>, Option<fastctx_content::v2::Lease>), Failure> {
    if encoded.len() < RECORD_BYTES {
        let mut body = Vec::with_capacity(encoded.len() + 1);
        body.push(EVENT);
        body.extend_from_slice(&encoded);
        return Ok((body, None));
    }
    let lease = library.put(&mut encoded.as_slice(), &format!("timeline:{resident}"), LEASE).map_err(failure)?;
    let mut body = vec![OVERSIZED];
    body.extend_from_slice(
        &serde_json::to_vec(&serde_json::json!({
            "bytes": encoded.len(),
            "slim": preview::preview(event),
            "likeness": super::identity::likeness(event, &encoded),
        }))
        .map_err(failure)?,
    );
    Ok((body, Some(lease)))
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
