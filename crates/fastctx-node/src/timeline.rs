//! A conversation's timeline as the content database keeps it: each event
//! of its history an item, so a viewer reads a page of it and is told when
//! it moves, where it once received the whole history.
//!
//! The items are the session's events as interfaces already draw them. They
//! are made by `build`, which reads the history as the World and the host
//! keep it and writes what changed.

pub(crate) mod api;
mod assets;
mod build;
pub(crate) mod copy;
pub(crate) mod last_page;
pub(crate) mod copied_changes;
pub(crate) mod copied_records;
pub(crate) mod records_known;
#[cfg(feature = "timeline-drill")]
mod drill;
mod identity;
mod later;
pub(crate) mod listed;
mod local;
mod artifacts;
mod attachments;
mod beside;
mod file_changes;
pub(crate) mod draft;
mod meta;
pub(crate) mod words;
pub(crate) use assets::{checkpoint_of, scope_of};
pub(crate) use attachments::placed_here as attached;
pub(crate) use identity::likeness;
pub(crate) mod names;
mod order;
mod preview;
pub(crate) mod reads;
mod serial;
pub(crate) mod spill;
pub(crate) mod viewed;
pub(crate) mod world;
pub(crate) mod born_shared;
mod tail;
#[cfg(feature = "bench")]
pub use tail::bench;
pub(crate) use assets::Early;
pub(crate) use tail::told_again;
pub(crate) use tail::forget_windows;
pub(crate) mod timers;

use fastctx_content::StreamKey;
use fastctx_jobs::{Job, Kind, Outcome, Taken};
use fastctx_types::ResidentId;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

/// The kind of job that brings a conversation's timeline to its history.
const KIND: &str = "timeline";
/// A build reads a conversation's whole history: few run at once.
const AT_ONCE: usize = 2;
/// The longest one build may take.
const WITHIN: Duration = Duration::from_secs(600);
/// Builds in a row that may fail before the job is set aside until asked again.
const TRIES: u32 = 5;
/// The soonest a conversation's timeline is built again after it was built:
/// while a turn runs its history changes with every item, and each build
/// reads all of it.
const APART: Duration = Duration::from_secs(15);

/// When each conversation's timeline was last built, in this run of the node.
static BUILT: LazyLock<Mutex<HashMap<ResidentId, Instant>>> = LazyLock::new(Mutex::default);
/// Builds in a row whose reading missed a record, by conversation: built
/// again until it reads whole, at most this many times (AA06).
static UNREAD: LazyLock<Mutex<HashMap<ResidentId, u32>>> = LazyLock::new(Mutex::default);
const UNREAD_TRIES: u32 = 20;
/// How long after the node starts the conversations are looked at again
/// (AA09).
const RESCAN_AFTER: Duration = Duration::from_secs(60);

/// The scope of what the content database keeps of a conversation.
pub(crate) fn scope(conversation: &str) -> String {
    format!("conversation/{conversation}")
}

/// The stream a conversation's timeline is kept in.
pub(crate) fn stream(resident: &ResidentId) -> StreamKey {
    StreamKey::new(scope(resident.as_str()), "timeline")
}

/// The name under which the build keeps how far a conversation's timeline
/// is built.
pub(crate) fn checkpoint(conversation: &str) -> String {
    format!("timeline/{conversation}")
}

/// What the asset library keeps beside a conversation's timeline as its
/// producer: where it stood, and the record ids before its seam.
pub(crate) fn asset_checkpoints(conversation: &str) -> [String; 2] {
    [format!("timeline/{conversation}"), format!("timeline-ids/{conversation}")]
}

/// The facts of a run `resident` tells of changed: its asset timeline is
/// brought to them, whatever else moves it (DD06).
pub(crate) fn runs_changed(node: &crate::Node, resident: &ResidentId) {
    tail::wanted_history(node, resident);
}

/// A network joined under a new term: every conversation here is brought to
/// its history again, the timelines of that network's into their new
/// holding periods whole (GG02); one of another network finds nothing to
/// do.
pub(crate) fn joined_anew(node: &crate::Node) {
    // Archived ones too: their metadata and words go into the new holding
    // period as well (J06).
    let here: Vec<ResidentId> = node.residents.everyone().filter(|record| record.here).map(|record| record.id).collect();
    for resident in &here {
        tail::wanted(node, resident);
    }
}

/// Names what builds timelines, before the node's jobs start.
pub(crate) fn register(node: &Arc<crate::Node>) {
    #[cfg(feature = "timeline-drill")]
    drill::start(node);
    produce(&Arc::downgrade(node));
    rescan(&Arc::downgrade(node));
    tail::register(node);
    tail::register_check(node);
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

/// Brings the timelines of the conversations whose history grew to it, read
/// or not: a block finished, words heard, a turn ended (design §10).
fn produce(node: &std::sync::Weak<crate::Node>) {
    let Some(watched) = node.upgrade().map(|node| node.watchers.produced.clone()) else { return };
    let node = node.clone();
    fastctx_platform::task::spawn(async move {
        loop {
            watched.notify.notified().await;
            let Some(node) = node.upgrade() else { return };
            // The asset timeline only: the content database's is made
            // again for a reader that asks for it (`wanted_content`), not
            // read whole as each history grows (design §28.53).
            for resident in watched.take() {
                tail::grew(&node, &resident);
                later::grew(&node, &resident);
            }
        }
    });
}

/// A while after the node starts, the timelines of the conversations active
/// lately are brought to their histories: what grew just before the node
/// stopped, its job not yet written, is not left behind (AA09).
fn rescan(node: &std::sync::Weak<crate::Node>) {
    let node = node.clone();
    fastctx_platform::task::spawn(async move {
        tokio::time::sleep(RESCAN_AFTER).await;
        let Some(node) = node.upgrade() else { return };
        // Archived ones are brought too — their metadata and words, whose
        // job may not have been written as the node stopped (J06) — and
        // their histories not checked.
        let archived: Vec<ResidentId> =
            node.residents.everyone().filter(|record| record.here && record.removed.is_some()).map(|record| record.id).collect();
        for resident in &archived {
            tail::wanted(&node, resident);
        }
        let here: Vec<ResidentId> =
            node.residents.everyone().filter(|record| record.here && record.removed.is_none()).map(|record| record.id).collect();
        // The asset timeline of each, whose version — what its history is
        // read from, as cheaply as it can be told — tells at once whether
        // anything is to do (BB06): one unchanged is not read, one changed
        // read on from where it was. None is checked whole for the node
        // having started (design §28.53: a change that hid itself from its
        // version is found by the week's check), nor the content database's
        // made again unless a reader asks.
        for resident in &here {
            tail::wanted(&node, resident);
        }
        drop(here);
        // And each one not checked for a week, every day after.
        let weak = std::sync::Arc::downgrade(&node);
        drop(node);
        loop {
            tokio::time::sleep(STALE_SWEEP).await;
            let Some(node) = weak.upgrade() else { return };
            tail::check_stale(&node).await;
        }
    });
}

/// How often the histories not checked for a week are looked for.
const STALE_SWEEP: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);

async fn run(node: &Arc<crate::Node>, resident: &ResidentId) -> Outcome {
    if node.residents.about(resident).is_none() {
        return Outcome::Done;
    }
    let since = BUILT.lock().expect("built timelines").get(resident).map(Instant::elapsed);
    if let Some(since) = since
        && since < APART
    {
        return Outcome::Again { after: APART - since };
    }
    // Read from the asset library by now, the content database's timeline
    // is made for no reader: one asked for before it was is not made, nor
    // its history read whole for it (design §28.52 52-1). A lab makes it
    // all the same (`api::here`).
    let assets = std::env::var_os("FASTCTX_LAB_CONTENT_TIMELINE").is_none()
        && fastctx_platform::roles::in_place("choosing a timeline", || reads::from_assets(node, resident)).unwrap_or(false);
    if assets {
        UNREAD.lock().expect("unread timelines").remove(resident);
        return Outcome::Done;
    }
    // Its asset timeline being made now, it is read there once made: asked
    // again after a while, by when it is, this finds nothing to do.
    if std::env::var_os("FASTCTX_LAB_CONTENT_TIMELINE").is_none() && tail::producing(resident) {
        return Outcome::Again { after: APART };
    }
    match build::build(node, resident).await {
        Ok(built) => {
            BUILT.lock().expect("built timelines").insert(resident.clone(), Instant::now());
            // A reading that missed a record is read again a while later:
            // the host may write its last lines late (AA06).
            if built.unread {
                let mut unread = UNREAD.lock().expect("unread timelines");
                let tries = unread.entry(resident.clone()).or_default();
                *tries += 1;
                if *tries < UNREAD_TRIES {
                    return Outcome::Again { after: APART };
                }
                unread.remove(resident);
            } else {
                UNREAD.lock().expect("unread timelines").remove(resident);
            }
            if built.written + built.gone > 0 || cfg!(feature = "timeline-drill") {
                node.log.line(format_args!(
                    "{resident}'s timeline: {} events, {} written, {} gone",
                    built.events, built.written, built.gone
                ));
            }
            Outcome::Done
        }
        Err(failure) => Outcome::Failed(failure.message),
    }
}

/// Asks for a conversation's metadata and where the user's words to it stand
/// in the asset library to be made again, its directory or its inbox having
/// changed. Its timeline's job makes them.
pub(crate) fn described(node: &crate::Node, resident: &ResidentId) {
    tail::wanted(node, resident);
}

/// Asks for a conversation's timeline in the content database alone to be
/// brought to its history, as a reading of it does. A reading never asks
/// for the asset library's: that one is made from what changes at its
/// source, whether anyone reads it or not (design §28.3).
pub(crate) fn wanted_content(node: &Arc<crate::Node>, resident: &ResidentId) {
    let job = Job { kind: KIND, key: resident.to_string(), unit: resident.to_string(), payload: Vec::new() };
    // Built a moment ago, it waits out the pause rather than being taken up
    // only to be put back again (AA12).
    let pause = BUILT
        .lock()
        .expect("built timelines")
        .get(resident)
        .map(Instant::elapsed)
        .filter(|since| *since < APART)
        .map_or(Duration::ZERO, |since| APART - since);
    let (database, jobs, log) = (node.database.clone(), node.durable_jobs.clone(), node.log.clone());
    fastctx_platform::task::spawn(async move {
        let bytes = job.key.len() * 2 + 64;
        let key = job.key.clone();
        match database.write_relaxed(bytes, move |db| fastctx_jobs::submit_after(db, &job, pause)).await {
            Ok(()) => jobs.stir(KIND),
            Err(error) => log.line(format_args!("asking for {key}'s timeline: {error}")),
        }
    });
}

impl crate::Node {
    /// The content database, when it opened.
    pub(crate) fn content(&self) -> Result<&fastctx_content::Content, fastctx_types::ipc::Failure> {
        self.content
            .as_ref()
            .ok_or_else(|| fastctx_types::ipc::Failure::new("this machine's content database is unavailable"))
    }
}
