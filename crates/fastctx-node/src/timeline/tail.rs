//! A conversation's timeline in the asset library, brought to its history as
//! the history grows (sync-v2-impl-design §10). A Claude resident's record
//! is read past where it was read through (`claude::history::window`) and
//! only that window is written: its new and changed items as revisions, its
//! items gone as tombstones, what came before left as written. Any other
//! history, and one that cannot be followed, is read and written whole.
//!
//! Where it stands is kept beside the timeline (`Following`), so a node that
//! stops takes up where it was. It runs as its own job, soon after the
//! history grows, apart from the content database's timeline, which a
//! reader reads.

use super::assets::{self, Span};
use crate::Node;
use fastctx_hosts::claude::transcript::follow::Seam;
use fastctx_jobs::{Job, Kind, Outcome, Taken};
use fastctx_types::ResidentId;
use fastctx_types::ipc::Failure;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

/// The kind of job that brings a conversation's timeline in the asset
/// library to its history.
const KIND: &str = "timeline-assets";
const AT_ONCE: usize = 2;
const WITHIN: Duration = Duration::from_secs(600);
const TRIES: u32 = 5;
/// The soonest it is brought again after it was.
const APART: Duration = Duration::from_secs(1);
/// A reading the last record kept is not in yet is tried again this often,
/// at most this many times in a row.
const UNREAD_AGAIN: Duration = Duration::from_secs(5);
const UNREAD_TRIES: u32 = 20;
/// The form of what is kept beside the timeline.
const FORM: u32 = 2;

static BROUGHT: LazyLock<Mutex<HashMap<ResidentId, Instant>>> = LazyLock::new(Mutex::default);
/// The asset timeline's builds, one per conversation at a time.
static PRODUCING: super::serial::Serial = super::serial::Serial::new();
/// While a conversation is at work, or just was, it is looked at again
/// this often: a host writes its record late, and says nothing when it does
/// (BB04).
const ACTIVE_AGAIN: Duration = Duration::from_secs(5);
const ACTIVE_FOR: Duration = Duration::from_secs(60);
/// A request to bring it that the database refused is asked again this
/// many times, this often (BB06).
const SUBMIT_TRIES: u32 = 5;
const SUBMIT_AGAIN: Duration = Duration::from_secs(5);
static UNREAD: LazyLock<Mutex<HashMap<ResidentId, u32>>> = LazyLock::new(Mutex::default);

/// Where a conversation's timeline in the asset library stands.
#[derive(Default, Serialize, Deserialize)]
struct Following {
    form: u32,
    /// The version of the history it was brought to (`held_history::version`).
    version: String,
    /// The session whose record is followed, and where it is read through.
    session: Option<String>,
    seam: Seam,
    /// The items of the window past the seam as last written: identity in
    /// hex, time, order key.
    window: Vec<(String, Timestamp, u64)>,
    /// The highest order key of the items before the window.
    floor: u64,
    /// The last record the session's mod saw kept that the file was found
    /// to hold.
    kept: Option<String>,
}

/// What bringing a timeline did.
#[derive(Default)]
pub(super) struct Produced {
    pub(super) written: usize,
    pub(super) gone: usize,
    /// The last record the session's mod saw kept is not in its file yet.
    pub(super) unread: bool,
}

fn failure(error: impl std::fmt::Display) -> Failure {
    Failure::new(error.to_string())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    (0..text.len()).step_by(2).map(|at| text.get(at..at + 2).and_then(|pair| u8::from_str_radix(pair, 16).ok())).collect()
}

/// Brings `resident`'s timeline in the asset library to its history, where
/// this machine keeps one.
pub(super) async fn produce(node: &Arc<Node>, resident: &ResidentId) -> Result<Produced, Failure> {
    let _producing = PRODUCING.hold(resident).await;
    let Some(kept) = node.assets.as_ref() else { return Ok(Produced::default()) };
    let Some((about, _)) = node.residents.about(resident) else { return Ok(Produced::default()) };
    let Some(placed) = assets::placed(node, &about)? else { return Ok(Produced::default()) };
    let library = kept.library.clone();
    let name = assets::checkpoint(resident);
    let state: Following = {
        let (reading, name) = (library.clone(), name.clone());
        let kept = fastctx_platform::task::blocking(move || reading.checkpoint(&name)).await.map_err(failure)?;
        kept.and_then(|bytes| serde_json::from_slice::<Following>(&bytes).ok()).filter(|state| state.form == FORM).unwrap_or_default()
    };
    let version = crate::held_history::version(node, resident).await?;
    if state.version == version {
        return Ok(Produced::default());
    }
    let directory = node.world.conversation(resident);
    let follows = about.host.agent.as_str() == "claude" && about.world_managed() && crate::logins::workbench::reads_host(node, &about);
    let window = if follows {
        let recorded = crate::talk::recorded_claude(&directory).await?;
        let from = state.session.clone().map(|session| (session, state.seam.clone()));
        crate::claude::history::window(node, resident, &recorded, from).await.map_err(Failure::new)?.map(|window| (window, recorded))
    } else {
        None
    };
    let (signer, hub) = (kept.signer(), placed.hub.clone());
    let (wrote, next, unread) = match window {
        Some((window, recorded)) => {
            let recorded: Vec<_> =
                if window.whole { recorded } else { recorded.into_iter().filter(|event| event.at >= window.since).collect() };
            let history = {
                let (assembling, whose, folder, events) = (node.clone(), resident.clone(), directory.clone(), window.events);
                fastctx_platform::task::blocking(move || crate::talk::assemble(&assembling, &whose, &folder, "claude", recorded, Some(events)))
                    .await?
            };
            let before: Vec<(Vec<u8>, Timestamp)> =
                state.window.iter().filter_map(|(identity, at, _)| unhex(identity).map(|identity| (identity, *at))).collect();
            // What the last window wrote from before this one began is settled.
            let floor = state.window.iter().filter(|(_, at, _)| *at < window.since).map(|(_, _, key)| *key).fold(state.floor, u64::max);
            let (whole, since, settled) = (window.whole, window.since, window.settled);
            let wrote = {
                let (writing, signer, whose, placed) = (library.clone(), signer.clone(), resident.clone(), placed);
                fastctx_platform::task::blocking(move || {
                    let span = if whole { Span::Whole } else { Span::From { since, before: &before, floor } };
                    assets::write(&writing, signer, &whose, &placed, &history, span)
                })
                .await?
            };
            // From where the seam stands now, the items before it are settled.
            let base = if whole { 0 } else { floor };
            let settled_floor = wrote.items.iter().filter(|(_, at, _)| *at < settled).map(|(_, _, key)| *key).fold(base, u64::max);
            let open = wrote.items.iter().filter(|(_, at, _)| *at >= settled).map(|(identity, at, key)| (hex(identity), *at, *key)).collect();
            let unread = window.kept.is_some() && !window.kept_here && window.kept != state.kept;
            let next = Following {
                form: FORM,
                // Brought again until the record holds what was kept.
                version: if unread { String::new() } else { version },
                session: Some(window.session),
                seam: window.seam,
                window: open,
                floor: settled_floor,
                kept: if window.kept_here { window.kept } else { state.kept },
            };
            (wrote, next, unread)
        }
        None => {
            let read = crate::talk::history_quietly(node, resident).await?;
            if read.unread {
                return Ok(Produced { unread: true, ..Produced::default() });
            }
            let wrote = {
                let (writing, signer, whose, history) = (library.clone(), signer.clone(), resident.clone(), read.history);
                fastctx_platform::task::blocking(move || assets::write(&writing, signer, &whose, &placed, &history, Span::Whole)).await?
            };
            let floor = wrote.items.iter().map(|(_, _, key)| *key).max().unwrap_or(0);
            (wrote, Following { form: FORM, version, floor, ..Following::default() }, false)
        }
    };
    let state = serde_json::to_vec(&next).map_err(failure)?;
    fastctx_platform::task::blocking(move || library.keep_checkpoint(&name, state)).await.map_err(failure)?;
    if wrote.written + wrote.gone > 0 {
        crate::assets::wake(node, &hub);
    }
    Ok(Produced { written: wrote.written, gone: wrote.gone, unread })
}

/// Names the job, before the node's jobs start.
pub(super) fn register(node: &Arc<Node>) {
    let jobs = node.durable_jobs.clone();
    let node = Arc::downgrade(node);
    let run = move |taken: Taken| -> std::pin::Pin<Box<dyn Future<Output = Outcome> + Send>> {
        let node = node.upgrade();
        Box::pin(async move {
            let Some(node) = node else { return Outcome::Failed("the node is stopping".into()) };
            match taken.unit.parse::<ResidentId>() {
                Ok(resident) => run(&node, &resident).await,
                Err(_) => Outcome::Done,
            }
        })
    };
    jobs.register(Kind { name: KIND, at_once: AT_ONCE, within: WITHIN, tries: TRIES }, Arc::new(run));
}

async fn run(node: &Arc<Node>, resident: &ResidentId) -> Outcome {
    if node.residents.about(resident).is_none() {
        return Outcome::Done;
    }
    let since = BROUGHT.lock().expect("brought timelines").get(resident).map(Instant::elapsed);
    if let Some(since) = since
        && since < APART
    {
        return Outcome::Again { after: APART - since };
    }
    let produced = produce(node, resident).await;
    BROUGHT.lock().expect("brought timelines").insert(resident.clone(), Instant::now());
    match produced {
        Ok(produced) => {
            if produced.written + produced.gone > 0 || cfg!(feature = "timeline-drill") {
                node.log.line(format_args!("{resident}'s asset timeline: {} written, {} gone", produced.written, produced.gone));
            }
            if produced.unread {
                let mut unread = UNREAD.lock().expect("unread timelines");
                let tries = unread.entry(resident.clone()).or_default();
                *tries += 1;
                if *tries < UNREAD_TRIES {
                    return Outcome::Again { after: UNREAD_AGAIN };
                }
                unread.remove(resident);
            } else {
                UNREAD.lock().expect("unread timelines").remove(resident);
            }
            if active(node, resident) {
                return Outcome::Again { after: ACTIVE_AGAIN };
            }
            Outcome::Done
        }
        Err(failure) => Outcome::Failed(failure.message),
    }
}

/// Whether `resident` is at work, or was a moment ago.
fn active(node: &Node, resident: &ResidentId) -> bool {
    node.residents.now(resident).is_some_and(|now| {
        now.turn.is_some()
            || now.activity == fastctx_types::residents::Activity::Busy
            || jiff::Timestamp::now().duration_since(now.since).unsigned_abs() < ACTIVE_FOR
    })
}

/// Asks for `resident`'s timeline in the asset library to be brought to its
/// history; asked again while it waits or runs, it is one job.
pub(super) fn wanted(node: &Arc<Node>, resident: &ResidentId) {
    if node.assets.is_none() {
        return;
    }
    let job = Job { kind: KIND, key: resident.to_string(), unit: resident.to_string(), payload: Vec::new() };
    let pause = BROUGHT
        .lock()
        .expect("brought timelines")
        .get(resident)
        .map(Instant::elapsed)
        .filter(|since| *since < APART)
        .map_or(Duration::ZERO, |since| APART - since);
    let (database, jobs, log) = (node.database.clone(), node.durable_jobs.clone(), node.log.clone());
    fastctx_platform::task::spawn(async move {
        let bytes = job.key.len() * 2 + 64;
        for tries in 1..=SUBMIT_TRIES {
            let asked = job.clone();
            match database.write_relaxed(bytes, move |db| fastctx_jobs::submit_after(db, &asked, pause)).await {
                Ok(()) => {
                    jobs.stir(KIND);
                    return;
                }
                Err(error) if tries == SUBMIT_TRIES => {
                    log.line(format_args!("asking for {}'s asset timeline: {error}", job.key));
                }
                Err(_) => tokio::time::sleep(SUBMIT_AGAIN).await,
            }
        }
    });
}
