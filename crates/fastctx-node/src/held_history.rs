//! A watch for a viewer that keeps the history on its own disk
//! (`fastctx_types::session::held`; `world-gui.md`, 对话). A viewer that
//! comes back with the mark of the last event it took is sent the events
//! after it, while they are all kept. One that holds nothing yet is sent the
//! segments the history was last cut into (`crate::cut_history`) and the
//! events told since. Otherwise the history is read again, slimmed to what an
//! interface draws, cut into segments, and the ones the viewer lacks are
//! sent; where the records the history is read from are as they were when the
//! viewer last read it, it says so without reading them whole.

use std::sync::Arc;

use fastctx_net::history::{self, Pictures};
use fastctx_types::ipc::Failure;
use fastctx_types::residents::Activity;
use fastctx_types::session::held::Held;
use fastctx_types::session::{SessionEvent, Watched};
use fastctx_types::{Address, ResidentId};
use sha2::{Digest as _, Sha256};

use crate::Node;
use crate::talk::Opening;

/// Named in every version, so that a viewer's copy made by an older way of
/// slimming or cutting is never taken for the history as it is sent now.
const FORM: &str = "fastctx history segments: turn boundaries, artifact identities and shared output text";

/// The first pieces of a watch of `resident` for a viewer that holds
/// `held`, and the events after them, slimmed.
pub(crate) async fn opening(node: &Arc<Node>, resident: &ResidentId, held: &Held) -> Result<Opening, Failure> {
    let owner = Address { machine: node.machine(), resident: resident.clone() };
    if let Some(since) = held.since {
        let followed = node.watchers.follow(resident, Some(since));
        if let Some(replay) = followed.replay.filter(|_| !followed.reset) {
            let (events, _) = crate::talk::requested(node, resident, followed.events)?;
            let mut pieces = vec![Watched::Resumed];
            pieces.extend(told(replay, &owner));
            return Ok(Opening { pieces: made(pieces), events, slim: Some(owner) });
        }
    }
    // A viewer that holds anything may hold some of the events told after
    // the cut, which would come to it twice.
    let empty = held.marks && held.version.is_none() && held.segments.is_empty();
    if empty && let Some(opening) = from_cut(node, resident, held, &owner).await? {
        return Ok(opening);
    }
    let reading = node.cut_histories.reading(resident);
    let _reading = reading.lock().await;
    if empty && let Some(opening) = from_cut(node, resident, held, &owner).await? {
        return Ok(opening);
    }
    // Subscribe before checking a held version: a rewrite between the check
    // and subscription must still reach the viewer that gets "unchanged".
    let subscribed = held.version.as_ref().map(|_| crate::talk::subscribe(node, resident)).transpose()?;
    // Read from the timeline here, what it holds is named too: corrected
    // since, its sources unchanged, it is not the history the viewer holds
    // (codex-e7b1552a P2-1).
    let stamped = stamp(node, resident).await?;
    let standing = crate::timeline::viewed::standing(node, resident)?;
    let version = stamped.clone().map(|stamped| versioned(stamped, standing));
    if let Some(version) = &version
        && held.version.as_ref() == Some(version)
    {
        let (events, pending) = subscribed.expect("a held version subscribed before its stamp");
        let mut pieces = vec![Watched::Unchanged { version: version.clone() }];
        pieces.extend(live(pending, &owner));
        return Ok(Opening { pieces: made(pieces), events, slim: Some(owner) });
    }
    drop(subscribed);
    let read = match from_timeline(node, resident, &owner).await? {
        Some(read) => read,
        None => from_sources(node, resident, &owner).await?,
    };
    // Never repeated: a viewer that holds it is sent the history next time.
    // So is a reading some record was missing from, which is read again
    // once it can be.
    let version = match stamped {
        Some(stamped) if !read.unread => versioned(stamped, read.revision),
        _ => format!("unstamped {:x}", u128::from_le_bytes(random()?)),
    };
    let mut pieces = crate::cut_history::pieces(&read.cut, held, version.clone());
    // Kept for the viewers that come next, unless some record was missing.
    if let Some(at) = read.at
        && !read.unread
    {
        node.cut_histories
            .keep(resident, crate::cut_history::CutHistory::new(at, version, read.cut.clone(), read.revision, read.snapshots.clone()));
    }
    pieces.extend(made(live(read.pending, &owner).collect()));
    pieces.extend(made(told(read.snapshots, &owner).collect()));
    Ok(Opening { pieces, events: read.events, slim: Some(owner) })
}

/// Pieces made already.
fn made(pieces: Vec<Watched>) -> Vec<crate::talk::OpeningPiece> {
    pieces.into_iter().map(crate::talk::OpeningPiece::Made).collect()
}

/// The version a viewer holds a history by: its sources' (`stamp`), and
/// where the timeline it was read from stood, where it was.
fn versioned(stamped: String, revision: Option<u64>) -> String {
    match revision {
        Some(revision) => format!("{stamped} timeline {revision:x}"),
        None => stamped,
    }
}

/// The history cut into segments as a viewer is sent it, slimmed, with the
/// events heard from where it was read and what was under way then.
struct Apart {
    cut: Arc<crate::cut_history::Kept>,
    /// Where the timeline it was read from stood; none, read from its
    /// sources.
    revision: Option<u64>,
    /// Some record of it could not be read now.
    unread: bool,
    pending: Vec<SessionEvent>,
    events: crate::watch::Events,
    snapshots: Vec<Arc<crate::watch::Marked>>,
    at: Option<fastctx_types::session::Mark>,
}

/// Slimmed for `owner`'s viewer, in the history's order.
async fn slimmed(node: &Arc<Node>, events: Vec<SessionEvent>, owner: &Address) -> Result<Vec<SessionEvent>, Failure> {
    let slimmed_for = owner.clone();
    let mut events = fastctx_platform::task::spawn_blocking(move || {
        let pictures = Pictures::Referenced { owner: &slimmed_for };
        let mut events = events;
        events.retain_mut(|event| history::slim(event, &pictures));
        events
    })
    .await
    .map_err(|error| Failure::new(format!("cutting the history into segments: {error}")))?;
    picture_fingerprints(node, &mut events).await?;
    Ok(events)
}

/// The history cut from the conversation's timeline as the asset library
/// holds it here, a page at a time (design §28.52 52-1): its segments,
/// slimmed, kept on disk as they are cut and read as they are sent; only
/// its outline and a page held, never the history whole nor its sources
/// read whole. None where the timeline is not read here, or an object of
/// it is not.
async fn from_timeline(node: &Arc<Node>, resident: &ResidentId, owner: &Address) -> Result<Option<Apart>, Failure> {
    use crate::timeline::viewed;
    if !node.residents.about(resident).is_some_and(|(about, _)| about.world_managed()) {
        return Ok(None);
    }
    // Of the machine's holding as the reading begins, however long it takes.
    let epoch = crate::cut_history::holding();
    // Heard first: the timeline brought after holds every event before.
    let followed = node.watchers.follow(resident, None);
    let Some((held, unread)) = viewed::brought(node, resident).await? else { return Ok(None) };
    let (events, pending) = crate::talk::requested(node, resident, followed.events)?;
    let mut reader = viewed::Reader::forward(held).with_runs(node.world.conversation(resident));
    let world = node.world.path().to_path_buf();
    let begin = || -> Result<(history::Segmenter, crate::cut_history::Writing), Failure> {
        let writing = crate::cut_history::Writing::create(&world, epoch).map_err(|error| Failure::new(format!("keeping the history's segments: {error}")))?;
        Ok((history::Segmenter::default(), writing))
    };
    let mut cutting = begin()?;
    let (mut tries, mut _steady) = (0, None);
    let failed = |error: String| Failure::new(format!("cutting the history into segments: {error}"));
    loop {
        // Each event slimmed as it is read, before the next is (P1-2).
        let slimmed_for = owner.clone();
        let (back, next) = fastctx_platform::task::spawn_blocking(move || {
            let pictures = Pictures::Referenced { owner: &slimmed_for };
            let next = reader.next_as(&mut |mut event| history::slim(&mut event, &pictures).then_some(event));
            (reader, next)
        })
        .await
        .map_err(|error| failed(error.to_string()))?;
        reader = back;
        match next? {
            viewed::Next::Page(mut page) => {
                picture_fingerprints(node, &mut page).await?;
                cutting = fastctx_platform::task::spawn_blocking(move || -> std::io::Result<_> {
                    let (mut segmenter, mut writing) = cutting;
                    for event in page {
                        for (segment, size) in segmenter.push(event).into_iter().flatten() {
                            writing.add(&segment, size)?;
                        }
                    }
                    Ok((segmenter, writing))
                })
                .await
                .map_err(|error| failed(error.to_string()))?
                .map_err(|error| failed(error.to_string()))?;
            }
            viewed::Next::End => break,
            viewed::Next::Changed => {
                tries += 1;
                if tries >= viewed::TRIES {
                    return Err(viewed::kept_changing(resident));
                }
                if tries + 1 == viewed::TRIES {
                    _steady = Some(viewed::steady(resident).await);
                }
                cutting = begin()?;
                reader.restart();
            }
            viewed::Next::Missing => return Ok(None),
        }
    }
    let cut = fastctx_platform::task::spawn_blocking(move || -> std::io::Result<crate::cut_history::Kept> {
        let (mut segmenter, mut writing) = cutting;
        if let Some((segment, size)) = segmenter.finish() {
            writing.add(&segment, size)?;
        }
        writing.finish()
    })
    .await
    .map_err(|error| failed(error.to_string()))?
    .map_err(|error| failed(error.to_string()))?;
    Ok(Some(Apart { cut: Arc::new(cut), revision: reader.revision(), unread, pending, events, snapshots: followed.snapshots, at: Some(followed.at) }))
}

/// The history cut from its sources read whole, as before the timeline was
/// read here.
async fn from_sources(node: &Arc<Node>, resident: &ResidentId, owner: &Address) -> Result<Apart, Failure> {
    let epoch = crate::cut_history::holding();
    let read = crate::talk::history_apart(node, resident).await?;
    let history = slimmed(node, read.history, owner).await?;
    let cut = fastctx_platform::task::spawn_blocking(move || crate::cut_history::Kept::held(history::cut(history), epoch))
        .await
        .map_err(|error| Failure::new(format!("cutting the history into segments: {error}")))?;
    Ok(Apart { cut: Arc::new(cut), revision: None, unread: read.unread, pending: read.pending, events: read.events, snapshots: read.snapshots, at: read.at })
}

/// The opening of a viewer that holds nothing, from the cut kept of the
/// history and the events told since, while they are all kept.
async fn from_cut(
    node: &Arc<Node>,
    resident: &ResidentId,
    held: &Held,
    owner: &Address,
) -> Result<Option<Opening>, Failure> {
    let Some(kept) = node.cut_histories.get(resident) else { return Ok(None) };
    // Cut from a timeline corrected since — its sources unchanged, nothing
    // told — it is not the history now.
    if kept.revision.is_some() && kept.revision != crate::timeline::viewed::standing(node, resident)? {
        node.cut_histories.forget(resident, &kept);
        return Ok(None);
    }
    let followed = node.watchers.follow(resident, Some(kept.at));
    let Some(replay) = followed.replay.filter(|_| !followed.reset) else {
        node.cut_histories.forget(resident, &kept);
        return Ok(None);
    };
    let (events, pending) = crate::talk::requested(node, resident, followed.events)?;
    let mut pieces = crate::cut_history::pieces(&kept.cut, held, kept.version.clone());
    pieces.extend(made(live(pending, owner).collect()));
    pieces.extend(made(told(kept.snapshots.clone(), owner).collect()));
    pieces.extend(made(told(replay, owner).collect()));
    Ok(Some(Opening { pieces, events, slim: Some(owner.clone()) }))
}

/// Events as the watchers told them, each with its mark, slimmed.
fn told(events: Vec<Arc<crate::watch::Marked>>, owner: &Address) -> impl Iterator<Item = Watched> + '_ {
    events.into_iter().filter_map(|marked| {
        let marked = Arc::unwrap_or_clone(marked);
        crate::talk::live(marked.event, Some(owner)).map(|event| Watched::Live {
            event,
            mark: marked.mark,
            likeness: None,
        })
    })
}

pub(crate) async fn picture_fingerprints(node: &Arc<Node>, events: &mut [SessionEvent]) -> Result<(), Failure> {
    for event in events {
        let fastctx_types::session::Happened::Heard { message: Some(message), .. } = &mut event.happened else {
            continue;
        };
        for image in &mut message.images {
            if image.content.is_some() {
                continue;
            }
            let file = match crate::attachment_files::resolve(
                node,
                image.source.clone(),
                Arc::new(std::sync::atomic::AtomicBool::new(false)),
            )
            .await
            {
                Ok(file) => file,
                Err(error) if error.code == Some(fastctx_types::ipc::FailureCode::OriginalGone) => continue,
                Err(error) => return Err(error),
            };
            image.content = Some(
                fastctx_platform::task::spawn_blocking(move || fastctx_hands::files::content_fingerprint(&file.path))
                    .await
                    .map_err(|error| Failure::new(error.to_string()))?
                    .map_err(|error| Failure::new(error.to_string()))?,
            );
        }
    }
    Ok(())
}

/// What the resident's host waits on someone to answer now, sent as it
/// happens rather than kept with the history, which outlives it.
fn live(pending: Vec<SessionEvent>, owner: &Address) -> impl Iterator<Item = Watched> + '_ {
    pending.into_iter().filter_map(|event| {
        crate::talk::live(event, Some(owner)).map(|event| Watched::Live { event, mark: None, likeness: None })
    })
}

/// A version of the resident's history, from what it is read from: equal
/// versions read the same history. None while a turn runs, when its host
/// adds to its record without saying so to any of these.
pub(crate) async fn stamp(node: &Arc<Node>, resident: &ResidentId) -> Result<Option<String>, Failure> {
    if node.residents.about(resident).is_none() {
        return Err(Failure::new(format!("no resident {resident} lives on {}", node.machine())));
    }
    if node.residents.now(resident).is_none_or(|now| now.turn.is_some() || now.activity == Activity::Busy) {
        return Ok(None);
    }
    version(node, resident).await.map(Some)
}

/// What the resident's history is read from, as one digest, whether or not a
/// turn runs: equal digests read the same history.
pub(crate) async fn version(node: &Arc<Node>, resident: &ResidentId) -> Result<String, Failure> {
    let (about, _) = node
        .residents
        .about(resident)
        .ok_or_else(|| Failure::new(format!("no resident {resident} lives on {}", node.machine())))?;
    let mut digest = Sha256::new();
    let mut take = |bytes: &[u8]| {
        digest.update((bytes.len() as u64).to_le_bytes());
        digest.update(bytes);
    };
    take(FORM.as_bytes());
    take(env!("CARGO_PKG_VERSION").as_bytes());
    take(&serde_json::to_vec(&about).map_err(|error| Failure::new(error.to_string()))?);
    let failed = |error: std::io::Error| Failure::new(format!("reading {resident}'s records: {error}"));
    let directory = node.world.conversation(resident);
    let records = fastctx_platform::task::spawn_blocking(move || {
        let mut records = Vec::new();
        let mut collect = |bytes: &[u8]| records.push(bytes.to_vec());
        crate::session_record::stamp(directory.path(), &mut collect)?;
        crate::run_facts::stamp(&directory, &mut collect)?;
        Ok::<_, std::io::Error>(records)
    })
    .await
    .map_err(|error| Failure::new(error.to_string()))?
    .map_err(failed)?;
    for record in records {
        take(&record);
    }
    // Where the World does not read the host, the history is the World's
    // record alone (`crate::talk`), and so is its version.
    if !crate::logins::workbench::reads_host(node, &about) {
        take(b"recorded alone");
        return Ok(digest.finalize().iter().map(|byte| format!("{byte:02x}")).collect());
    }
    match (about.host.agent.as_str(), &about.session) {
        ("codex", Some(thread)) => {
            let codex = {
                // Finding its host may read its host mode from its disk.
                let (finding, host) = (node.clone(), about.clone());
                fastctx_platform::task::blocking(move || finding.codex.of(&finding, &host)).await?
            };
            take(codex.stamp(thread).await.map_err(Failure::new)?.as_bytes());
        }
        ("claude", _) => {
            // Its sessions' records are looked at on disk.
            let (reading, whose) = (node.clone(), resident.clone());
            let stamped = fastctx_platform::task::blocking(move || {
                let mut stamped = Vec::new();
                crate::claude::history::stamp(&reading, &whose, &mut |bytes: &[u8]| stamped.push(bytes.to_vec()));
                stamped
            })
            .await;
            for bytes in stamped {
                take(&bytes);
            }
        }
        _ => {}
    }
    Ok(digest.finalize().iter().map(|byte| format!("{byte:02x}")).collect())
}

fn random() -> Result<[u8; 16], Failure> {
    fastctx_net::sign::random::<16>().map_err(|error| Failure::new(format!("naming a version: {error}")))
}

/// The host's own record of one item of a resident's conversation on any
/// machine.
pub(crate) async fn item_detail(
    node: &Arc<Node>,
    resident: ResidentId,
    machine: Option<fastctx_types::access::Written>,
    turn: Option<String>,
    item: String,
) -> Result<(Address, Option<serde_json::Value>), Failure> {
    let local = machine.is_none() && node.residents.about(&resident).is_some();
    let machine = match machine {
        Some(machine) => machine,
        None if node.residents.about(&resident).is_some() => node.machine().into(),
        None => crate::colleagues::find_for(node, &fastctx_types::access::Acting::Here, &resident).await?,
    };
    if local || node.is_here(&machine) {
        let detail = item_detail_here(node, &resident, turn.as_deref(), &item).await?;
        return Ok((Address { machine: machine.name, resident }, detail));
    }
    let call = fastctx_types::remote::RemoteCall::ItemDetail { resident: resident.clone(), turn, item };
    let written = &machine;
    match node
        .call(&fastctx_types::access::Acting::Here, written, fastctx_types::access::Purpose::Agents, call)
        .await
        .map_err(|failure| Failure::new(failure.to_string()))?
    {
        Ok(fastctx_types::remote::RemoteReply::ItemDetail { detail }) => {
            Ok((Address { machine: machine.name, resident }, detail))
        }
        Ok(_) => Err(Failure::new(format!("{machine} answered something other than an item's record"))),
        Err(failure) => Err(failure),
    }
}

/// The host's own record of one item of a resident of this machine, read
/// out of its whole history as it is now.
pub(crate) async fn item_detail_here(
    node: &Arc<Node>,
    resident: &ResidentId,
    turn: Option<&str>,
    item: &str,
) -> Result<Option<serde_json::Value>, Failure> {
    if let Some(found) = item_detail_in_timeline(node, resident, turn, item).await? {
        return Ok(found);
    }
    let read = crate::talk::history_quietly(node, resident).await?;
    Ok(read.history.into_iter().rev().find(|event| names(event, turn, item)).map(detail))
}

/// Whether `event` is the item `item` (of `turn`, where it is asked) whose
/// record is asked for.
fn names(event: &SessionEvent, turn: Option<&str>, item: &str) -> bool {
    use fastctx_types::session::Happened;
    match &event.happened {
        Happened::Finished { turn: at, item: found } | Happened::Updated { turn: at, item: found } => {
            turn.is_none_or(|turn| turn == at) && found.id == item
        }
        Happened::Heard { message: Some(message), turn: at, .. } => {
            turn.is_none_or(|turn| at.as_deref() == Some(turn)) && message.id == item
        }
        _ => false,
    }
}

/// The host's record an event `names` holds.
fn detail(event: SessionEvent) -> serde_json::Value {
    use fastctx_types::session::Happened;
    match event.happened {
        Happened::Finished { item, .. } | Happened::Updated { item, .. } => item.detail.unwrap_or(serde_json::Value::Null),
        Happened::Heard { message: Some(message), .. } => message.detail,
        _ => serde_json::Value::Null,
    }
}

/// The record of the last event of the conversation's timeline that is the
/// item, read back from its end a page at a time (design §28.52 52-1): an
/// event too large for a record is fetched whole only where what is shown
/// in its place is the item (`preview` keeps what tells events apart).
/// None where the timeline is not read here, or an object of it is not.
async fn item_detail_in_timeline(
    node: &Arc<Node>,
    resident: &ResidentId,
    turn: Option<&str>,
    item: &str,
) -> Result<Option<Option<serde_json::Value>>, Failure> {
    use crate::timeline::viewed::{self, Next};
    use fastctx_client::timeline_copy::Shown;
    if !node.residents.about(resident).is_some_and(|(about, _)| about.world_managed()) {
        return Ok(None);
    }
    let Some((held, _)) = viewed::brought(node, resident).await? else { return Ok(None) };
    let (turn, item) = (turn.map(str::to_string), item.to_string());
    /// What a page read back from the end found.
    enum Found {
        Detail(serde_json::Value),
        Read(Next<Shown>),
    }
    let mut reader = viewed::Reader::backward(held);
    let (mut tries, mut _steady) = (0, None);
    loop {
        let (asked_turn, asked_item) = (turn.clone(), item.clone());
        let (back, found) = fastctx_platform::task::spawn_blocking(move || {
            let found = (|| -> Result<Found, Failure> {
                let shown = match reader.next_shown()? {
                    Next::Page(shown) => shown,
                    other => return Ok(Found::Read(other)),
                };
                for shown in shown {
                    let candidate = match &shown {
                        Shown::Event(encoded) => serde_json::from_slice::<SessionEvent>(encoded).ok(),
                        Shown::Oversized { slim: Some(slim), .. } => serde_json::from_slice::<SessionEvent>(slim).ok(),
                        Shown::Oversized { slim: None, .. } => None,
                    };
                    if !candidate.as_ref().is_none_or(|event| names(event, asked_turn.as_deref(), &asked_item)) {
                        continue;
                    }
                    let whole = match (&shown, candidate) {
                        (Shown::Event(_), Some(event)) => event,
                        _ => match reader.whole(&shown)? {
                            Some(event) => event,
                            None => return Ok(Found::Read(if reader.moved()? { Next::Changed } else { Next::Missing })),
                        },
                    };
                    if names(&whole, asked_turn.as_deref(), &asked_item) {
                        return Ok(Found::Detail(detail(whole)));
                    }
                }
                Ok(Found::Read(Next::Page(Vec::new())))
            })();
            (reader, found)
        })
        .await
        .map_err(|error| Failure::new(error.to_string()))?;
        reader = back;
        match found? {
            Found::Detail(detail) => return Ok(Some(Some(detail))),
            Found::Read(Next::Page(_)) => {}
            Found::Read(Next::End) => return Ok(Some(None)),
            Found::Read(Next::Changed) => {
                tries += 1;
                if tries >= viewed::TRIES {
                    return Err(viewed::kept_changing(resident));
                }
                if tries + 1 == viewed::TRIES {
                    _steady = Some(viewed::steady(resident).await);
                }
                reader.restart();
            }
            Found::Read(Next::Missing) => return Ok(None),
        }
    }
}
