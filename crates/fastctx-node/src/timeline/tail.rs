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
use sha2::{Digest, Sha256};
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
/// The form of what is kept beside the timeline: one of an earlier form is
/// read whole again — of form 2, a seam might be kept with no record ids
/// before it (EE06).
const FORM: u32 = 4;
/// The checks of whole histories (§10): the kind of job, within how long
/// of a change past a long prefix one runs, how often each history is
/// checked at least, and the share of a core they take at most.
const VERIFY: &str = "timeline-verify";
const LONG_PREFIX: u64 = 32 << 20;
const LONG_WITHIN: Duration = Duration::from_secs(60 * 60);
const VERIFY_EVERY: Duration = Duration::from_secs(7 * 24 * 60 * 60);
const VERIFY_SHARE: u32 = 10;
/// Checks are begun within this long of being asked for, and one begun
/// later than this after is late (R5).
const CHECK_SPREAD: Duration = Duration::from_secs(30 * 60);
const CHECK_LATE: Duration = Duration::from_secs(40 * 60);
/// How long a conversation come from elsewhere waits for the hub's
/// description of its timeline before it is begun here (AG10).
const SCOPE_WAIT: Duration = Duration::from_secs(120);
static SCOPE_LOOKED: LazyLock<Mutex<HashMap<ResidentId, Instant>>> = LazyLock::new(Mutex::default);
/// Until when checks rest after the last one (DD09).
static REST_UNTIL: LazyLock<Mutex<Option<Instant>>> = LazyLock::new(Mutex::default);
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
/// After those, it is asked again this often until the database takes it
/// (CC04): one asking at a time for each conversation.
const SUBMIT_LATER: Duration = Duration::from_secs(60);
static SUBMITTING_LATER: LazyLock<Mutex<std::collections::HashSet<ResidentId>>> = LazyLock::new(Mutex::default);
/// A timeline another device orders is tried again this often while its
/// ordering is handed over.
const HANDING_AGAIN: Duration = Duration::from_secs(3);
/// A conversation fallen quiet is looked at again for this long, more often
/// for the first part (CC03).
const QUIET_SOON: Duration = Duration::from_secs(10 * 60);
const QUIET_FOR: Duration = Duration::from_secs(60 * 60);
static UNREAD: LazyLock<Mutex<HashMap<ResidentId, u32>>> = LazyLock::new(Mutex::default);

/// Where a conversation's timeline in the asset library stands.
#[derive(Default, Serialize, Deserialize)]
struct Following {
    form: u32,
    /// The version of the history it was brought to (`held_history::version`).
    version: String,
    /// The holding period of the conversation's scope it was brought to in:
    /// held anew since, it is brought again whole (GG02).
    #[serde(default)]
    holding: Option<i64>,
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
    /// A Claude resident's sessions before the one followed, as their files
    /// stood.
    #[serde(default)]
    earlier: Vec<crate::claude::history::Stamp>,
    /// Before when the history was settled when it was last read.
    #[serde(default)]
    settled_at: Option<Timestamp>,
    /// The facts of the runs the conversation tells of, as they stood.
    #[serde(default)]
    runs: String,
    /// The items of the window told again at every reading (a Codex
    /// thread's title and what it says of itself), in hex.
    #[serde(default)]
    told: Vec<String>,
}

/// Whether a conversation come from elsewhere still waits for the hub's
/// description of its timeline: for `SCOPE_WAIT` from the first time it was
/// looked for, in this run of the node.
fn waited_for_scope(resident: &ResidentId) -> bool {
    let mut first = SCOPE_LOOKED.lock().expect("scopes looked for");
    first.entry(resident.clone()).or_insert_with(Instant::now).elapsed() < SCOPE_WAIT
}

/// The holding period the producer last finished making the timeline in,
/// as kept beside it; none while it is set back for a whole making, or was
/// never made (JJ05).
pub(super) fn made_in(state: &[u8]) -> Option<i64> {
    serde_json::from_slice::<Following>(state).ok().and_then(|following| following.holding)
}

/// What is kept beside the timeline, set back before a whole write (DD03):
/// a stop after the write and before what it leaves is kept reads whole
/// again, not past a floor below which it wrote.
/// Held by the build (EE05): a build given up on never writes it after a
/// newer one.
async fn reset(library: &fastctx_content::v2::Library, name: &str, held: super::serial::Held) -> Result<(), Failure> {
    let state = serde_json::to_vec(&Following { form: FORM, ..Following::default() }).map_err(failure)?;
    let (writing, name) = (library.clone(), name.to_string());
    fastctx_platform::task::blocking(move || {
        let _held = held;
        writing.keep_checkpoint(&name, state)
    })
    .await
    .map_err(failure)
}

/// The name the record ids before a Claude seam are kept under.
fn prefix_name(resident: &ResidentId) -> String {
    format!("timeline-ids/{resident}")
}

/// The record ids before `resident`'s Claude seam, as kept.
async fn prefix_ids(library: &fastctx_content::v2::Library, resident: &ResidentId) -> Result<Arc<crate::claude::history::PrefixIds>, Failure> {
    let (reading, name) = (library.clone(), prefix_name(resident));
    let bytes = fastctx_platform::task::blocking(move || reading.checkpoint(&name)).await.map_err(failure)?.unwrap_or_default();
    let digests = bytes.chunks_exact(8).map(|chunk| u64::from_le_bytes(chunk.try_into().expect("eight bytes"))).collect();
    Ok(Arc::new(crate::claude::history::PrefixIds(digests)))
}

/// Keeps the record ids the seam moved past with those before: all there
/// are, read whole. Held by the build, as `reset` is (EE05).
async fn keep_prefix_ids(
    library: &fastctx_content::v2::Library,
    resident: &ResidentId,
    whole: bool,
    moved: &[String],
    held: super::serial::Held,
) -> Result<(), Failure> {
    if !whole && moved.is_empty() {
        return Ok(());
    }
    let mut digests = if whole { Vec::new() } else { Arc::try_unwrap(prefix_ids(library, resident).await?).map(|ids| ids.0).unwrap_or_default() };
    digests.extend(moved.iter().map(|id| crate::claude::history::PrefixIds::digest(id)));
    digests.sort_unstable();
    digests.dedup();
    let bytes: Vec<u8> = digests.iter().flat_map(|digest| digest.to_le_bytes()).collect();
    let (writing, name) = (library.clone(), prefix_name(resident));
    fastctx_platform::task::blocking(move || {
        let _held = held;
        writing.keep_checkpoint(&name, bytes)
    })
    .await
    .map_err(failure)
}

/// The identities of the timeline's items placed past `floor`.
fn past_floor(library: &fastctx_content::v2::Library, scope: &str, floor: u64) -> fastctx_content::Result<Vec<Vec<u8>>> {
    use fastctx_content::v2::{Kind as _, Timeline};
    let mut items = Vec::new();
    // Past every identity at the floor's own place.
    let mut after = Some((floor.to_be_bytes().to_vec(), vec![0xff; 33]));
    loop {
        let page = library.page(scope, Timeline::STREAM, after.clone(), fastctx_content::v2::PAGE)?;
        let Some(last) = page.last() else { return Ok(items) };
        after = Some((last.order.clone(), last.item.clone()));
        items.extend(page.into_iter().map(|entry| entry.item));
    }
}

/// The facts of the runs `directory` tells of, as they stand.
async fn runs_of(directory: &fastctx_types::layout::ConversationDir) -> Result<String, Failure> {
    let reading = directory.clone();
    let stamped = fastctx_platform::task::spawn_blocking(move || {
        let mut digest = Vec::new();
        crate::run_facts::stamp(&reading, &mut |bytes: &[u8]| digest.extend_from_slice(bytes))?;
        Ok::<_, std::io::Error>(digest)
    })
    .await
    .map_err(|error| Failure::new(error.to_string()))?
    .map_err(failure)?;
    Ok(hex(&Sha256::digest(&stamped)))
}

/// A history read past where it was read through, as either host reads it.
struct Followed {
    events: Vec<SessionEvent>,
    /// The events its host tells again at every reading, wherever their
    /// time falls: a Codex thread's title and what it says of itself, a
    /// Claude session's title and note of what could not be read.
    told: Vec<SessionEvent>,
    whole: bool,
    since: Timestamp,
    settled: Timestamp,
    next: Next,
}

/// Where the history is read through now, as its host reads it.
enum Next {
    Claude {
        session: String,
        seam: Box<Seam>,
        kept: Option<String>,
        kept_here: bool,
        earlier: Vec<crate::claude::history::Stamp>,
        prefix_ids: Vec<String>,
    },
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
    /// Another device orders the timeline: written once it is handed over.
    pub(super) handing: bool,
}

fn failure(error: impl std::fmt::Display) -> Failure {
    Failure::new(error.to_string())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The bytes `hex` wrote, if `text` is such.
fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len()).step_by(2).map(|at| u8::from_str_radix(text.get(at..at + 2)?, 16).ok()).collect()
}


/// Brings `resident`'s timeline in the asset library to its history, where
/// this machine keeps one.
pub(super) async fn produce(node: &Arc<Node>, resident: &ResidentId, whole: bool) -> Result<Produced, Failure> {
    let producing = PRODUCING.hold(resident).await;
    let Some(kept) = node.assets.as_ref() else { return Ok(Produced::default()) };
    let Some((about, _)) = node.residents.about(resident) else { return Ok(Produced::default()) };
    let Some(placed) = assets::placed(node, &about)? else { return Ok(Produced::default()) };
    let library = kept.library.clone();
    let name = assets::checkpoint(resident);
    let state: Following = {
        let (reading, name, held) = (library.clone(), name.clone(), producing.clone());
        let kept = fastctx_platform::task::blocking(move || {
            let _held = held;
            reading.checkpoint(&name)
        })
        .await
        .map_err(failure)?;
        kept.and_then(|bytes| serde_json::from_slice::<Following>(&bytes).ok()).filter(|state| state.form == FORM).unwrap_or_default()
    };
    // Held anew since — the network left and joined again, the scope
    // revoked and given again — what was made here is of another holding
    // period, whose past may be gone with it: it is made again whole, the
    // history unchanged or not (GG02).
    let holding = {
        let (asking, scope) = (library.clone(), placed.scope.clone());
        fastctx_platform::task::blocking(move || asking.holding(&scope)).await.map_err(failure)?
    };
    let (state, whole) = if state.holding.is_some() && state.holding != holding { (Following::default(), true) } else { (state, whole) };
    let version = crate::held_history::version(node, resident).await?;
    if state.version == version && !whole {
        // Unchanged: when it was last checked still stands (DD10).
        return Ok(Produced { verified: state.verified, ..Produced::default() });
    }
    // Come from another machine, the timeline is not begun here before the
    // hub's description of it is: the hub may have it already, ordered
    // there (AG10). Waited for a while; one the hub never had is begun here
    // after that.
    // Born under a name this machine had is born here (AH04).
    let born_elsewhere = node.residents.birth_machine(resident).is_some_and(|born| !node.is_me(&born));
    if born_elsewhere {
        let (asking, scope) = (library.clone(), placed.scope.clone());
        let live = fastctx_platform::task::blocking(move || asking.is_live(&scope)).await.map_err(failure)?;
        if !live && waited_for_scope(resident) {
            crate::assets::wake(node, &placed.hub);
            return Ok(Produced { handing: true, ..Produced::default() });
        }
    }
    // Let go to another machine with its export: written there from where
    // it ended, not here (J08).
    if crate::assets::release::released_away(node.world.conversation(resident).path(), &placed.scope, &kept.device()) {
        return Ok(Produced::default());
    }
    // Ordered by another device — the conversation came here from it — the
    // timeline goes on here once the hub hands its ordering over (§8).
    let orderer = {
        let (asking, scope) = (library.clone(), placed.scope.clone());
        fastctx_platform::task::blocking(move || asking.orderer(&scope, <fastctx_content::v2::Timeline as fastctx_content::v2::Kind>::STREAM)).await.map_err(failure)?
    };
    if orderer.is_some_and(|orderer| orderer != kept.device()) {
        crate::assets::takeover::want(node, &placed.hub, &placed.scope, <fastctx_content::v2::Timeline as fastctx_content::v2::Kind>::STREAM);
        return Ok(Produced { handing: true, ..Produced::default() });
    }
    let directory = node.world.conversation(resident);
    let runs = runs_of(&directory).await?;
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
        let heard = |sequence: &u64| state.world_latest.is_none_or(|latest| *sequence > latest);
        let replaced = agent == "codex" && numbered.iter().any(|(sequence, event)| heard(sequence) && replaced_history(event));
        // A fact the World tells late, of a time before what is settled, is
        // placed only by reading all again (CC15).
        let late = numbered.iter().any(|(sequence, event)| heard(sequence) && state.settled_at.is_some_and(|settled| event.at < settled));
        let mut open = state.world_open.clone();
        let mut whole = whole || replaced || late;
        // A reading of the whole history reads the World's record whole too
        // (R1): what the World alone tells of turns before where it was read
        // through would otherwise be written gone.
        let mut from_start = state.world_from.is_none() || cut || whole;
        if from_start {
            if state.world_from.is_some() {
                (numbered, latest, _) = crate::talk::recorded_after(&directory, 0).await?;
            }
            open.clear();
        }
        let followed = loop {
            let mut recorded = open.clone();
            recorded.extend(numbered.iter().map(|(_, event)| event.clone()));
            let followed = if agent == "claude" {
                // A check reads the record from its start.
                let from = state.session.clone().filter(|_| !whole).map(|session| (session, state.seam.clone(), state.earlier.clone()));
                let prefix = if whole { Arc::new(crate::claude::history::PrefixIds::default()) } else { prefix_ids(&library, resident).await? };
                crate::claude::history::window(node, resident, &recorded, from, prefix).await.map_err(Failure::new)?.map(|window| Followed {
                    events: window.events,
                    told: window.told,
                    whole: window.whole,
                    since: window.since,
                    settled: window.settled,
                    next: Next::Claude {
                        session: window.session,
                        seam: Box::new(window.seam),
                        kept: window.kept,
                        kept_here: window.kept_here,
                        earlier: window.earlier,
                        prefix_ids: window.prefix_ids,
                    },
                })
            } else {
                let from = state.codex.clone().filter(|_| !whole);
                crate::codex::follow::window(node, resident, &about, &recorded, from).await.map_err(Failure::new)?.map(|window| Followed {
                    told: window.events.iter().filter(|event| told_again(event)).cloned().collect(),
                    events: window.events,
                    whole: window.whole,
                    since: window.since,
                    settled: window.settled,
                    next: Next::Codex(window.seam),
                })
            };
            // The host read whole of its own accord (a seam gone, a settled
            // turn changed) while the World's record was read only past
            // where it was: read again, both whole (R1).
            match followed {
                Some(followed) if followed.whole && !from_start => {
                    (numbered, latest, _) = crate::talk::recorded_after(&directory, 0).await?;
                    open.clear();
                    from_start = true;
                    whole = true;
                }
                followed => break followed.map(|followed| (followed, recorded)),
            }
        };
        followed.map(|(followed, recorded)| (followed, recorded, numbered, open, latest))
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
            // The host's own title and what it says of the thread, told again
            // at every reading (R3: not a title the World recorded).
            let host_told: Vec<serde_json::Value> = window.told.iter().filter_map(|event| serde_json::to_value(event).ok()).collect();
            let history = {
                let (assembling, whose, folder, events, agent) = (node.clone(), resident.clone(), directory.clone(), window.events, agent.to_string());
                let held = producing.clone();
                fastctx_platform::task::blocking(move || {
                    let _held = held;
                    crate::talk::assemble(&assembling, &whose, &folder, &agent, recorded, Some(events))
                })
                .await?
            };
            // What the last window wrote from before this one began is settled
            // — not what is told again at every reading, whatever its time
            // (R2): its place stays where it is, below the window or in it.
            let floor = state
                .window
                .iter()
                .filter(|(identity, at, _)| *at < window.since && !state.told.contains(identity))
                .map(|(_, _, key)| *key)
                .fold(state.floor, u64::max);
            // What the window wrote before, as the library holds it: every
            // item placed past the floor, whatever the checkpoint kept of it
            // (CC12) — a write committed before a stop that kept no
            // checkpoint is among them — and what the host told at the last
            // reading, wherever a whole writing placed it, below the floor
            // too. Each is read again in this window, or gone.
            let before: Vec<(Vec<u8>, Timestamp)> = if window.whole {
                Vec::new()
            } else {
                let (reading, scope, held) = (library.clone(), placed.scope.clone(), producing.clone());
                let since = window.since;
                let told: Vec<Vec<u8>> = state.told.iter().filter_map(|identity| unhex(identity)).collect();
                fastctx_platform::task::blocking(move || {
                    let _held = held;
                    past_floor(&reading, &scope, floor).map(|mut items| {
                        for identity in told {
                            if !items.contains(&identity) {
                                items.push(identity);
                            }
                        }
                        items.into_iter().map(|identity| (identity, since)).collect()
                    })
                })
                .await
                .map_err(failure)?
            };
            let (whole, since, settled) = (window.whole, window.since, window.settled);
            // What the host tells again at every reading stays in the window,
            // whatever its time (a Codex thread's last change, a Claude
            // session's start): never settled below the floor, it goes when
            // a reading no longer tells it (EE01).
            let unturned: std::collections::HashSet<Vec<u8>> = {
                let identities = super::names::identities(&history);
                history
                    .iter()
                    .zip(identities)
                    .filter(|(event, _)| serde_json::to_value(event).is_ok_and(|value| host_told.contains(&value)))
                    .map(|(_, identity)| identity)
                    .collect()
            };
            // A Codex thread's own title and what it says of itself stand at
            // the end of its history: placed again where they stand (N1).
            let again: std::collections::HashSet<Vec<u8>> = if matches!(window.next, Next::Codex(_)) { unturned.clone() } else { Default::default() };
            if whole {
                reset(&library, &name, producing.clone()).await?;
            }
            // Read on from where it stood, it is written only in the holding
            // period it stood in.
            let expected = if whole { None } else { state.holding };
            let wrote = {
                let (writing, signer, whose, placed, held) = (library.clone(), signer.clone(), resident.clone(), placed, producing.clone());
                fastctx_platform::task::blocking(move || {
                    let _held = held;
                    let span = if whole { Span::Whole } else { Span::From { since, before: &before, floor } };
                    assets::write(&writing, signer, &whose, &placed, &history, span, &again, expected)
                })
                .await?
            };
            // From where the seam stands now, the items before it are settled.
            // Held in the window: what is from where the seam stands on, what
            // is told again at every reading, and an event new to the
            // timeline placed in the window though its time is earlier (DD04)
            // — written past the floor, it goes when the history drops it.
            let held = |identity: &Vec<u8>, at: &Timestamp| *at >= settled || *at < since || unturned.contains(identity);
            let base = if whole { 0 } else { floor };
            let settled_floor = wrote.items.iter().filter(|(identity, at, _)| !held(identity, at)).map(|(_, _, key)| *key).fold(base, u64::max);
            let open = wrote.items.iter().filter(|(identity, at, _)| held(identity, at)).map(|(identity, at, key)| (hex(identity), *at, *key)).collect();
            let told: Vec<String> = unturned.iter().map(|identity| hex(identity)).collect();
            let verified = if window.whole { Some(Timestamp::now()) } else { state.verified };
            let (next, unread) = match window.next {
                Next::Claude { session, seam, kept: seen, kept_here, earlier, prefix_ids: moved_past } => {
                    keep_prefix_ids(&library, resident, whole, &moved_past, producing.clone()).await?;
                    let unread = seen.is_some() && !kept_here && seen != state.kept;
                    let next = Following {
                        form: FORM,
                        // Brought again until the record holds what was kept.
                        version: if unread { String::new() } else { version },
                        holding: Some(wrote.holding),
                        session: Some(session),
                        seam: *seam,
                        window: open,
                        told,
                        floor: settled_floor,
                        kept: if kept_here { seen } else { state.kept },
                        world_from: Some(world_from),
                        world_open,
                        verified,
                        codex: None,
                        world_latest: Some(latest),
                        earlier,
                        settled_at: Some(settled),
                        runs: runs.clone(),
                    };
                    (next, unread)
                }
                Next::Codex(seam) => {
                    let next = Following {
                        form: FORM,
                        version,
                        holding: Some(wrote.holding),
                        window: open,
                        told,
                        floor: settled_floor,
                        world_from: Some(world_from),
                        world_open,
                        verified,
                        codex: Some(seam),
                        world_latest: Some(latest),
                        settled_at: Some(settled),
                        runs: runs.clone(),
                        ..Following::default()
                    };
                    (next, false)
                }
            };
            (wrote, next, unread)
        }
        None => {
            let read = crate::talk::history_quietly(node, resident).await?;
            // A source unreadable now, gone for good or not yet whole, holds
            // up none of the rest: what was read is written, with the note
            // of what could not be, and nothing is taken away until a
            // reading of every source has it gone; it is read again later.
            let unread = read.unread;
            reset(&library, &name, producing.clone()).await?;
            let wrote = {
                let (writing, signer, whose, history, held) = (library.clone(), signer.clone(), resident.clone(), read.history, producing.clone());
                fastctx_platform::task::blocking(move || {
                    let _held = held;
                    let span = if unread { Span::Kept } else { Span::Whole };
                    assets::write(&writing, signer, &whose, &placed, &history, span, &Default::default(), None)
                })
                .await?
            };
            let floor = wrote.items.iter().map(|(_, _, key)| *key).max().unwrap_or(0);
            let following = Following {
                form: FORM,
                version: if unread { String::new() } else { version },
                holding: Some(wrote.holding),
                floor,
                verified: if unread { state.verified } else { Some(Timestamp::now()) },
                runs: runs.clone(),
                ..Following::default()
            };
            (wrote, following, unread)
        }
    };
    // A history read past a seam that changed is checked whole within the
    // hour (§10, §21): a long Claude prefix is read past only in part, a
    // Codex thread's settled turns are not read again, and what settled
    // as it was read may be told otherwise after (CC08–CC17); so are the
    // runs it tells of, whose facts go on after their item settled (CC15).
    let followed = next.session.is_some() || next.codex.is_some();
    let long_changed =
        next.seam.offset > LONG_PREFIX || followed && wrote.written + wrote.gone > 0 || followed && runs != state.runs && !state.runs.is_empty();
    let verified = next.verified;
    let state = serde_json::to_vec(&next).map_err(failure)?;
    let held = producing.clone();
    fastctx_platform::task::blocking(move || {
        let _held = held;
        library.keep_checkpoint(&name, state)
    })
    .await
    .map_err(failure)?;
    if wrote.written + wrote.gone > 0 {
        crate::assets::wake(node, &hub);
    }
    Ok(Produced { written: wrote.written, gone: wrote.gone, unread, long_changed, verified, handing: false })
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
            // How long it waited since it was asked for: one late takes half
            // a core while it catches up (§10, R5).
            let asked = taken.payload.get(..16).and_then(|bytes| bytes.try_into().ok()).map(i128::from_le_bytes);
            let waited = asked
                .and_then(|asked| Timestamp::from_nanosecond(asked).ok())
                .map(|asked| Timestamp::now().duration_since(asked).unsigned_abs());
            let late = waited.is_some_and(|waited| waited >= CHECK_LATE);
            // Resting from the last check: begun when the rest is over, the
            // rest outside any job's time (DD09) — but no later than when it
            // would be late, so it is done within the hour it was asked for
            // whatever rest another left (EE07).
            let resting = REST_UNTIL.lock().expect("checks' rest").and_then(|until| until.checked_duration_since(Instant::now()));
            if let Some(rest) = resting.filter(|_| !late) {
                let until_late = waited.map_or(CHECK_LATE, |waited| CHECK_LATE.saturating_sub(waited));
                return Outcome::Again { after: rest.min(until_late).max(Duration::from_secs(1)) };
            }
            // Begun: a change from now on asks for another (R4) — not
            // before, so one asked while it rested keeps when it was asked
            // (EE08).
            CHECK_ASKED.lock().expect("checks asked").remove(&resident);
            let began = Instant::now();
            let checked = produce(&node, &resident, true).await;
            let took = began.elapsed();
            match checked {
                // Not all could be read: not checked, tried again (DD08).
                Ok(checked) if checked.unread => Outcome::Again { after: UNREAD_AGAIN * 12 },
                Ok(checked) => {
                    if checked.written + checked.gone > 0 {
                        node.log.line(format_args!(
                            "{resident}'s asset timeline checked whole: {} written, {} gone",
                            checked.written, checked.gone
                        ));
                    }
                    let rest = if late { took } else { took * (VERIFY_SHARE - 1) };
                    if late {
                        node.log.line(format_args!("{resident}'s check began late: checks are behind and take half a core"));
                    }
                    *REST_UNTIL.lock().expect("checks' rest") = Instant::now().checked_add(rest);
                    Outcome::Done
                }
                Err(failure) => Outcome::Failed(failure.message),
            }
        })
    };
    jobs.register(Kind { name: VERIFY, at_once: 1, within: WITHIN, tries: TRIES }, Arc::new(run));
}

/// Asks for `resident`'s history to be checked whole, begun within the
/// first half of the next hour so it is done within it (R5); asked once
/// while one waits (a check begun frees the asking, R4).
pub(super) fn check_wanted(node: &Arc<Node>, resident: &ResidentId) {
    {
        let mut asked = CHECK_ASKED.lock().expect("checks asked");
        if asked.get(resident).is_some_and(|at| at.elapsed() < LONG_WITHIN) {
            return;
        }
        asked.insert(resident.clone(), Instant::now());
    }
    // Spread over that half hour, by the conversation, so checks asked
    // together do not run together.
    let spread = {
        let digest = Sha256::digest(resident.as_str().as_bytes());
        u64::from_le_bytes(digest[..8].try_into().expect("eight bytes")) % CHECK_SPREAD.as_nanos() as u64
    };
    let pause = Duration::from_nanos(spread);
    let asked = Timestamp::now().as_nanosecond().to_le_bytes().to_vec();
    let job = Job { kind: VERIFY, key: resident.to_string(), unit: resident.to_string(), payload: asked };
    let (database, jobs, log) = (node.database.clone(), node.durable_jobs.clone(), node.log.clone());
    fastctx_platform::task::spawn(async move {
        let bytes = job.key.len() * 2 + 64;
        let key = job.key.clone();
        // Refused, it is asked again each minute until it is taken (N5).
        loop {
            let asked = job.clone();
            match database.write_relaxed(bytes, move |db| fastctx_jobs::submit_after(db, &asked, pause)).await {
                Ok(()) => {
                    jobs.stir(VERIFY);
                    return;
                }
                Err(error) => {
                    log.line(format_args!("asking for {key}'s history to be checked: {error}; asked again in a minute"));
                    tokio::time::sleep(SUBMIT_LATER).await;
                }
            }
        }
    });
}

/// Asks for a check of each history here not checked for a week (§10, R4).
pub(super) async fn check_stale(node: &Arc<Node>) {
    let Some(kept) = node.assets.as_ref() else { return };
    let here: Vec<ResidentId> =
        node.residents.everyone().filter(|record| record.here && record.removed.is_none()).map(|record| record.id).collect();
    for resident in here {
        let (reading, name) = (kept.library.clone(), assets::checkpoint(&resident));
        let state = fastctx_platform::task::blocking(move || reading.checkpoint(&name)).await.ok().flatten();
        let verified = state.and_then(|bytes| serde_json::from_slice::<Following>(&bytes).ok()).and_then(|state| state.verified);
        // A day ahead, so none waits past a week for the next sweep (N4).
        if verified.is_none_or(|at| Timestamp::now().duration_since(at).unsigned_abs() >= VERIFY_EVERY - Duration::from_secs(24 * 60 * 60)) {
            check_wanted(node, &resident);
        }
    }
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
    // Its metadata beside it, made again from its directory (plan 3-4); and
    // where the user's words to it stand, followed from its inbox (§28.14).
    // Either failing, the job comes again soon (AP14).
    let mut beside_failed = false;
    match super::meta::produce(node, resident, &PRODUCING).await {
        Ok(0) => {}
        Ok(written) => node.log.line(format_args!("{resident}'s metadata: {written} written")),
        Err(error) => {
            beside_failed = true;
            node.log.line(format_args!("{resident}'s metadata: {error}"));
        }
    }
    match super::words::produce(node, resident, &PRODUCING).await {
        Ok(0) => {}
        Ok(written) => node.log.line(format_args!("{resident}'s words: {written} written")),
        Err(error) => {
            beside_failed = true;
            node.log.line(format_args!("{resident}'s words: {error}"));
        }
    }
    BROUGHT.lock().expect("brought timelines").insert(resident.clone(), Instant::now());
    match produced {
        Ok(produced) if produced.handing => Outcome::Again { after: HANDING_AGAIN },
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
            if beside_failed {
                return Outcome::Again { after: UNREAD_AGAIN };
            }
            // A host writes its record late, by no bound it tells, and says
            // nothing when it does (CC03): looked at again, less and less
            // often, for an hour after the conversation fell quiet; past
            // that, its checks find what came later.
            if let Some(after) = quiet_again(node, resident) {
                return Outcome::Again { after };
            }
            Outcome::Done
        }
        Err(failure) => Outcome::Failed(failure.message),
    }
}

/// How soon a conversation fallen quiet is looked at again: every half
/// minute for ten minutes, then every five, until an hour has gone by.
fn quiet_again(node: &Node, resident: &ResidentId) -> Option<Duration> {
    let quiet = node.residents.now(resident).map(|now| jiff::Timestamp::now().duration_since(now.since).unsigned_abs())?;
    if quiet < QUIET_SOON {
        Some(Duration::from_secs(30))
    } else if quiet < QUIET_FOR {
        Some(Duration::from_secs(300))
    } else {
        None
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
pub(super) fn wanted(node: &Node, resident: &ResidentId) {
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
                    log.line(format_args!("asking for {}'s asset timeline: {error}; asked again each minute", job.key));
                    let Ok(resident) = job.unit.parse::<ResidentId>() else { return };
                    if !SUBMITTING_LATER.lock().expect("submitting later").insert(resident.clone()) {
                        return;
                    }
                    loop {
                        tokio::time::sleep(SUBMIT_LATER).await;
                        let asked = job.clone();
                        if database.write_relaxed(bytes, move |db| fastctx_jobs::submit_after(db, &asked, Duration::ZERO)).await.is_ok() {
                            jobs.stir(KIND);
                            break;
                        }
                    }
                    SUBMITTING_LATER.lock().expect("submitting later").remove(&resident);
                }
                Err(_) => tokio::time::sleep(SUBMIT_AGAIN).await,
            }
        }
    });
}
