//! A conversation's timeline in the asset library, brought to its history as
//! the history grows (sync-v2-impl-design §10). A Claude resident's record,
//! or a Codex resident's thread, is read past where it was read through
//! (`claude::history::window`, `codex::follow::window`) and only that window
//! is written: its new and changed items as revisions, its items gone as
//! tombstones, what came before left as written. Any other history, and one
//! that cannot be followed, is read and written whole.
//!
//! Where it stands is kept beside the timeline (`Following`), so a node that
//! stops takes up where it was. It runs as its own job, soon after the
//! history grows, apart from the content database's timeline, which a
//! reader reads.

use super::assets::{self, Span};
use crate::Node;
use fastctx_hosts::claude::transcript::follow::Seam;
use fastctx_hosts::codex::follow::Seam as CodexSeam;
use fastctx_jobs::{Job, Kind, Outcome, Taken};
use fastctx_types::ResidentId;
use fastctx_types::ipc::Failure;
use fastctx_types::session::SessionEvent;
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
/// The checks of whole histories (§10): the kind of job, within how long
/// of a change past a long prefix one runs, how often each history is
/// checked at least, and the share of a core they take at most.
const VERIFY: &str = "timeline-verify";
const LONG_PREFIX: u64 = 32 << 20;
const LONG_WITHIN: Duration = Duration::from_secs(60 * 60);
const VERIFY_EVERY: Duration = Duration::from_secs(7 * 24 * 60 * 60);
const VERIFY_SHARE: u32 = 10;
/// When each history's check was last asked for, in this run of the node.
static CHECK_ASKED: LazyLock<Mutex<HashMap<ResidentId, Instant>>> = LazyLock::new(Mutex::default);

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
    /// How far the World's own record was read: what comes after it is read
    /// at the next window, nothing before (design §18: the World read is
    /// what is new). None before it was read once.
    world_from: Option<u64>,
    /// The starts of the World's turns open where its record was read
    /// through: the window's events are placed in them.
    world_open: Vec<SessionEvent>,
    /// When the whole history was last read and compared (`verify`).
    verified: Option<Timestamp>,
    /// Where a Codex thread is read through.
    #[serde(default)]
    codex: Option<CodexSeam>,
    /// The World's record's latest sequence when it was last read.
    #[serde(default)]
    world_latest: Option<u64>,
}

/// A history read past where it was read through, as either host reads it.
struct Followed {
    events: Vec<SessionEvent>,
    whole: bool,
    since: Timestamp,
    settled: Timestamp,
    next: Next,
}

/// Where the history is read through now, as its host reads it.
enum Next {
    Claude { session: String, seam: Seam, kept: Option<String>, kept_here: bool },
    Codex(CodexSeam),
}

/// Whether `event` is one a Codex reading tells again each time: the
/// thread's title, or what it says of itself.
fn told_again(event: &SessionEvent) -> bool {
    use fastctx_types::session::{Happened, SessionChange};
    matches!(event.happened, Happened::Titled { .. } | Happened::Changed { turn: None, change: SessionChange::Metadata { .. } })
}

/// Whether `event` says Codex put a history in place of the one it had.
fn replaced_history(event: &SessionEvent) -> bool {
    use fastctx_types::session::{Happened, SessionChange};
    matches!(event.happened, Happened::Changed { change: SessionChange::HistoryChanged, .. })
}

/// What bringing a timeline did.
#[derive(Default)]
pub(super) struct Produced {
    pub(super) written: usize,
    pub(super) gone: usize,
    /// The last record the session's mod saw kept is not in its file yet.
    pub(super) unread: bool,
    /// The history changed past a prefix too long to check whole at each
    /// advance (`follow`): it is checked whole within the hour (§10).
    pub(super) long_changed: bool,
    /// When the whole history was last read and compared.
    pub(super) verified: Option<Timestamp>,
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
pub(super) async fn produce(node: &Arc<Node>, resident: &ResidentId, whole: bool) -> Result<Produced, Failure> {
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
    if state.version == version && !whole {
        return Ok(Produced::default());
    }
    let directory = node.world.conversation(resident);
    let agent = about.host.agent.as_str();
    let hosted = about.world_managed() && crate::logins::workbench::reads_host(node, &about);
    let follows = hosted && (agent == "claude" || agent == "codex" && about.session.is_some());
    let window = if follows {
        // What Codex said so far is in the World's record before it is read.
        if agent == "codex"
            && let Some(thread) = &about.session
        {
            let codex = node.codex.of(node, &about)?;
            codex.flush_events(thread).await.map_err(Failure::new)?;
        }
        // The World's record past where it was read; read whole the first
        // time, or when a cut took turns back, or Codex put its history in
        // place of another.
        let (mut numbered, mut latest, cut) = crate::talk::recorded_after(&directory, state.world_from.unwrap_or(0)).await?;
        // Only one not heard before: the window reads again what it read.
        let replaced = agent == "codex"
            && numbered.iter().any(|(sequence, event)| state.world_latest.is_none_or(|latest| *sequence > latest) && replaced_history(event));
        let mut open = state.world_open.clone();
        let mut whole = whole;
        if state.world_from.is_none() || cut || replaced {
            if state.world_from.is_some() {
                (numbered, latest, _) = crate::talk::recorded_after(&directory, 0).await?;
            }
            open.clear();
            whole |= replaced;
        }
        let mut recorded = open.clone();
        recorded.extend(numbered.iter().map(|(_, event)| event.clone()));
        let followed = if agent == "claude" {
            // A check reads the record from its start.
            let from = state.session.clone().filter(|_| !whole).map(|session| (session, state.seam.clone()));
            crate::claude::history::window(node, resident, &recorded, from).await.map_err(Failure::new)?.map(|window| Followed {
                events: window.events,
                whole: window.whole,
                since: window.since,
                settled: window.settled,
                next: Next::Claude { session: window.session, seam: window.seam, kept: window.kept, kept_here: window.kept_here },
            })
        } else {
            let from = state.codex.clone().filter(|_| !whole);
            crate::codex::follow::window(node, resident, &about, &recorded, from).await.map_err(Failure::new)?.map(|window| Followed {
                events: window.events,
                whole: window.whole,
                since: window.since,
                settled: window.settled,
                next: Next::Codex(window.seam),
            })
        };
        followed.map(|followed| (followed, recorded, numbered, open, latest))
    } else {
        None
    };
    let (signer, hub) = (kept.signer(), placed.hub.clone());
    let (wrote, next, unread) = match window {
        Some((window, recorded, numbered, open, latest)) => {
            let (world_from, world_open) = world_after(&numbered, open, latest, window.settled, state.world_from);
            let recorded: Vec<_> =
                if window.whole { recorded } else { recorded.into_iter().filter(|event| event.at >= window.since).collect() };
            // Codex: reverted turns and requests stay out, as a reading has it.
            let recorded = if agent == "codex" { crate::codex::watch::recorded(recorded, &window.events) } else { recorded };
            let history = {
                let (assembling, whose, folder, events, agent) = (node.clone(), resident.clone(), directory.clone(), window.events, agent.to_string());
                fastctx_platform::task::blocking(move || crate::talk::assemble(&assembling, &whose, &folder, &agent, recorded, Some(events)))
                    .await?
            };
            let before: Vec<(Vec<u8>, Timestamp)> =
                state.window.iter().filter_map(|(identity, at, _)| unhex(identity).map(|identity| (identity, *at))).collect();
            // A Codex window is read again from the turn it began with: all
            // it wrote last is in it again, or gone. A Claude one from where
            // its seam moved: what it wrote from before this one began is
            // settled.
            let before: Vec<(Vec<u8>, Timestamp)> = match &window.next {
                Next::Codex(_) => before.into_iter().map(|(identity, at)| (identity, at.max(window.since))).collect(),
                Next::Claude { .. } => before,
            };
            let floor = state.window.iter().filter(|(_, at, _)| *at < window.since).map(|(_, _, key)| *key).fold(state.floor, u64::max);
            let (whole, since, settled) = (window.whole, window.since, window.settled);
            // A Codex thread's title and what it says of itself stay in the
            // window: every reading tells them again, at the time of the
            // thread's last change, not where they stand.
            let unturned: std::collections::HashSet<Vec<u8>> = {
                let identities = super::names::identities(&history);
                history.iter().zip(identities).filter(|(event, _)| told_again(event)).map(|(_, identity)| identity).collect()
            };
            let wrote = {
                let (writing, signer, whose, placed) = (library.clone(), signer.clone(), resident.clone(), placed);
                fastctx_platform::task::blocking(move || {
                    let span = if whole { Span::Whole } else { Span::From { since, before: &before, floor } };
                    assets::write(&writing, signer, &whose, &placed, &history, span)
                })
                .await?
            };
            // From where the seam stands now, the items before it are settled.
            let codex = matches!(window.next, Next::Codex(_));
            let held = |identity: &Vec<u8>, at: &Timestamp| *at >= settled || codex && unturned.contains(identity);
            let base = if whole { 0 } else { floor };
            let settled_floor = wrote.items.iter().filter(|(identity, at, _)| !held(identity, at)).map(|(_, _, key)| *key).fold(base, u64::max);
            let open = wrote.items.iter().filter(|(identity, at, _)| held(identity, at)).map(|(identity, at, key)| (hex(identity), *at, *key)).collect();
            let verified = if window.whole { Some(Timestamp::now()) } else { state.verified };
            let (next, unread) = match window.next {
                Next::Claude { session, seam, kept: seen, kept_here } => {
                    let unread = seen.is_some() && !kept_here && seen != state.kept;
                    let next = Following {
                        form: FORM,
                        // Brought again until the record holds what was kept.
                        version: if unread { String::new() } else { version },
                        session: Some(session),
                        seam,
                        window: open,
                        floor: settled_floor,
                        kept: if kept_here { seen } else { state.kept },
                        world_from: Some(world_from),
                        world_open,
                        verified,
                        codex: None,
                        world_latest: Some(latest),
                    };
                    (next, unread)
                }
                Next::Codex(seam) => {
                    let next = Following {
                        form: FORM,
                        version,
                        window: open,
                        floor: settled_floor,
                        world_from: Some(world_from),
                        world_open,
                        verified,
                        codex: Some(seam),
                        world_latest: Some(latest),
                        ..Following::default()
                    };
                    (next, false)
                }
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
            (wrote, Following { form: FORM, version, floor, verified: Some(Timestamp::now()), ..Following::default() }, false)
        }
    };
    // A long Claude prefix changed, or a Codex thread did, whose settled
    // turns are checked whole within the hour (§10, §21).
    let long_changed = next.seam.offset > LONG_PREFIX || next.codex.is_some() && wrote.written + wrote.gone > 0;
    let verified = next.verified;
    let state = serde_json::to_vec(&next).map_err(failure)?;
    fastctx_platform::task::blocking(move || library.keep_checkpoint(&name, state)).await.map_err(failure)?;
    if wrote.written + wrote.gone > 0 {
        crate::assets::wake(node, &hub);
    }
    Ok(Produced { written: wrote.written, gone: wrote.gone, unread, long_changed, verified })
}

/// Where the World's record is read through once the events before
/// `settled` are settled, given the events read past where it was, `open`
/// the starts of the turns open there, and `latest`: before the first event
/// of `settled` or later, the starts of the turns not ended by then kept.
fn world_after(
    numbered: &[(u64, SessionEvent)],
    open: Vec<SessionEvent>,
    latest: u64,
    settled: Timestamp,
    before: Option<u64>,
) -> (u64, Vec<SessionEvent>) {
    use fastctx_types::session::Happened;
    let through = numbered
        .iter()
        .filter(|(_, event)| event.at >= settled)
        .map(|(sequence, _)| sequence.saturating_sub(1))
        .min()
        .unwrap_or_else(|| latest.max(before.unwrap_or(0)));
    let ended: std::collections::HashSet<&str> = numbered
        .iter()
        .filter(|(_, event)| event.at <= settled)
        .filter_map(|(_, event)| match &event.happened {
            Happened::TurnEnded { turn, .. } => Some(turn.as_str()),
            _ => None,
        })
        .collect();
    let started = open.into_iter().chain(numbered.iter().filter(|(sequence, _)| *sequence <= through).map(|(_, event)| event.clone()));
    let open = started
        .filter(|event| matches!(&event.happened, Happened::TurnStarted { turn } if !ended.contains(turn.as_str())))
        .collect();
    (through, open)
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

/// Names the check of whole histories, before the node's jobs start: one at
/// a time, each followed by a rest nine times as long as it took, so checks
/// take a tenth of a core at most (§10).
pub(super) fn register_check(node: &Arc<Node>) {
    let jobs = node.durable_jobs.clone();
    let node = Arc::downgrade(node);
    let run = move |taken: Taken| -> std::pin::Pin<Box<dyn Future<Output = Outcome> + Send>> {
        let node = node.upgrade();
        Box::pin(async move {
            let Some(node) = node else { return Outcome::Failed("the node is stopping".into()) };
            let Ok(resident) = taken.unit.parse::<ResidentId>() else { return Outcome::Done };
            if node.residents.about(&resident).is_none() {
                return Outcome::Done;
            }
            let began = Instant::now();
            let checked = produce(&node, &resident, true).await;
            let took = began.elapsed();
            match checked {
                Ok(checked) => {
                    if checked.written + checked.gone > 0 {
                        node.log.line(format_args!(
                            "{resident}'s asset timeline checked whole: {} written, {} gone",
                            checked.written, checked.gone
                        ));
                    }
                    tokio::time::sleep(took * (VERIFY_SHARE - 1)).await;
                    Outcome::Done
                }
                Err(failure) => Outcome::Failed(failure.message),
            }
        })
    };
    jobs.register(Kind { name: VERIFY, at_once: 1, within: WITHIN, tries: TRIES }, Arc::new(run));
}

/// Asks for `resident`'s history to be checked whole, at a moment of the
/// next hour; asked at most once an hour in this run of the node.
fn check_wanted(node: &Arc<Node>, resident: &ResidentId) {
    {
        let mut asked = CHECK_ASKED.lock().expect("checks asked");
        if asked.get(resident).is_some_and(|at| at.elapsed() < LONG_WITHIN) {
            return;
        }
        asked.insert(resident.clone(), Instant::now());
    }
    // Spread over the hour, so checks asked together do not run together.
    let spread = (Timestamp::now().as_nanosecond().unsigned_abs() % LONG_WITHIN.as_nanos()) as u64;
    let pause = Duration::from_nanos(spread);
    let job = Job { kind: VERIFY, key: resident.to_string(), unit: resident.to_string(), payload: Vec::new() };
    let (database, jobs, log) = (node.database.clone(), node.durable_jobs.clone(), node.log.clone());
    fastctx_platform::task::spawn(async move {
        let bytes = job.key.len() * 2 + 64;
        let key = job.key.clone();
        match database.write_relaxed(bytes, move |db| fastctx_jobs::submit_after(db, &job, pause)).await {
            Ok(()) => jobs.stir(VERIFY),
            Err(error) => log.line(format_args!("asking for {key}'s history to be checked: {error}")),
        }
    });
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
    let produced = produce(node, resident, false).await;
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
            // A long prefix changed: checked whole within the hour. Any
            // history not checked for a week, or not since this node began
            // keeping it: checked, at some moment of the next hour.
            let stale = produced.verified.is_none_or(|at| Timestamp::now().duration_since(at).unsigned_abs() >= VERIFY_EVERY);
            if produced.long_changed || stale {
                check_wanted(node, resident);
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
