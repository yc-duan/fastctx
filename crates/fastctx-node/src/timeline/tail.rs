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

use super::assets::{self, Reach, Span};
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
/// How long before its job's time a check ends itself (`register_check`).
const CHECK_ENDS_BEFORE: Duration = Duration::from_secs(30);
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
const FORM: u32 = 5;
/// The checks of whole histories (§10, §28.53): the kind of job, within how
/// long of a change of its runs' facts one runs, how often each history is
/// checked at least, and the share of a core they take at most.
const VERIFY: &str = "timeline-verify";
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
/// Until when checks rest after the last one (DD09): one in time until
/// nine times as long as the last took, one late until as long (§10, R5;
/// codex-e024d399 #6: each late one rests too, so late ones together take
/// half a core, not all of one).
static REST_UNTIL: LazyLock<Mutex<Option<Instant>>> = LazyLock::new(Mutex::default);
static LATE_REST_UNTIL: LazyLock<Mutex<Option<Instant>>> = LazyLock::new(Mutex::default);
/// When each history's check was last asked for, in this run of the node.
static CHECK_ASKED: LazyLock<Mutex<HashMap<ResidentId, Instant>>> = LazyLock::new(Mutex::default);

static BROUGHT: LazyLock<Mutex<HashMap<ResidentId, Instant>>> = LazyLock::new(Mutex::default);
/// The asset timeline's builds, one per conversation at a time.
pub(super) static PRODUCING: super::serial::Serial = super::serial::Serial::new();

/// Whether the asset library's timeline of `resident` is being made now: it
/// is read there in a moment, and its history is not read whole for the
/// content database's beside it (`api::here`, `timeline::run`).
pub(super) fn producing(resident: &ResidentId) -> bool {
    PRODUCING.busy(resident)
}

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
    /// The generation the items of the window past the seam as last
    /// written are kept under beside the checkpoint (`Held`, design §28.51
    /// ⑤-7): each one's time and order key, those told again left out.
    #[serde(default)]
    held: Option<u64>,
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
    /// The version of the facts of the runs the conversation tells of that
    /// every item of the timeline has (`run_facts::version`); none where
    /// that is not known, and the history is to be read whole for them.
    #[serde(default)]
    runs_version: Option<i64>,
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
/// newer one. Kept only in the `epoch` it was read in (`publish`).
async fn reset(library: &fastctx_content::v2::Library, name: &str, epoch: u64, held: super::serial::Held) -> Result<(), Failure> {
    let state = serde_json::to_vec(&Following { form: FORM, ..Following::default() }).map_err(failure)?;
    publish(library, name, state, epoch, held).await
}

/// Keeps `state` as where the timeline `name` stands, only while what it
/// stands on was not let go since it was read (in `epoch`: a network's
/// sync that discarded what this machine wrote, the conversation gone):
/// then this reading is let go too, and the next reads anew (design §28.51
/// ⑤-7). Held by the build, as `reset` is.
async fn publish(library: &fastctx_content::v2::Library, name: &str, state: Vec<u8>, epoch: u64, held: super::serial::Held) -> Result<(), Failure> {
    let (writing, name) = (library.clone(), name.to_string());
    let kept = fastctx_platform::task::blocking(move || {
        let _held = held;
        // Let go meanwhile, what this reading kept beside the checkpoint
        // goes with what was let go: no checkpoint names it, and the
        // conversation may never be read again to sweep it.
        let kept = writing.keep_checkpoint_in(&name, state, epoch)?;
        if !kept {
            writing.sweep_held(&name, None)?;
        }
        Ok::<_, fastctx_content::Error>(kept)
    })
    .await
    .map_err(failure)?;
    if !kept {
        return Err(Failure::new("what the timeline was read on was let go meanwhile; it is read anew".to_string()));
    }
    Ok(())
}

/// The name the record ids before a Claude seam are kept under.
fn prefix_name(resident: &ResidentId) -> String {
    format!("timeline-ids/{resident}")
}

/// The generation the record ids before `resident`'s Claude seam are kept
/// under beside the checkpoint (`Library::stage_digests`, design §28.51
/// ⑤-7c): its own checkpoint is that number alone.
async fn prefix_generation(library: &fastctx_content::v2::Library, resident: &ResidentId) -> Result<Option<u64>, Failure> {
    let (reading, name) = (library.clone(), prefix_name(resident));
    let bytes = fastctx_platform::task::blocking(move || reading.checkpoint(&name)).await.map_err(failure)?;
    Ok(bytes.and_then(|bytes| <[u8; 8]>::try_from(bytes.as_slice()).ok()).map(u64::from_le_bytes))
}

/// How many digests of record ids are kept beside the checkpoint at once.
const DIGESTS_AT_ONCE: usize = 4096;

/// Keeps the record ids the seam moved past with those before — kept beside
/// the checkpoint as their 8-byte digests (`PrefixIds::digest`), asked of a
/// batch at a time, never read whole (design §28.51 ⑤-7c): read whole, all
/// there are, under a generation of their own, which the checkpoint then
/// names; read on past a seam, those it moved past joining the generation
/// named. `moved` as digests, or, read into a work store, `sorted`, the
/// checkpoint's bytes made there (eight little-endian bytes each). Kept
/// before where the timeline stands is (a stop between the two reads the
/// seam where it was, and a row of these past it reads all whole). Held by
/// the build, as `reset` is (EE05).
async fn keep_prefix_ids(
    library: &fastctx_content::v2::Library,
    resident: &ResidentId,
    whole: bool,
    moved: &[u64],
    sorted: Option<Vec<u8>>,
    held: super::serial::Held,
) -> Result<(), Failure> {
    let digests: Vec<u64> = match &sorted {
        Some(bytes) => bytes.chunks_exact(8).map(|chunk| u64::from_le_bytes(chunk.try_into().expect("eight bytes"))).collect(),
        None => moved.to_vec(),
    };
    drop(sorted);
    if !whole && digests.is_empty() {
        return Ok(());
    }
    let named = prefix_generation(library, resident).await?;
    let (writing, name) = (library.clone(), prefix_name(resident));
    fastctx_platform::task::blocking(move || {
        let _held = held;
        stage_prefix(&writing, &name, named, whole, &digests)
    })
    .await
    .map_err(failure)?;
    Ok(())
}

/// `keep_prefix_ids` beside the checkpoint `name`, whose generation is
/// `named`: the generation they are kept under.
fn stage_prefix(library: &fastctx_content::v2::Library, name: &str, named: Option<u64>, whole: bool, digests: &[u64]) -> fastctx_content::Result<u64> {
    let (generation, fresh) = match named {
        Some(named) if !whole => (named, false),
        named => (named.map_or(1, |named| named.wrapping_add(1)), true),
    };
    let mut first = fresh;
    for batch in digests.chunks(DIGESTS_AT_ONCE) {
        library.stage_digests(name, generation, first, batch.to_vec())?;
        first = false;
    }
    if first {
        library.stage_digests(name, generation, true, Vec::new())?;
    }
    if fresh {
        library.keep_checkpoint_settled(name, generation.to_le_bytes().to_vec())?;
        library.sweep_digests(name, Some(generation))?;
    }
    Ok(generation)
}

/// The identities of the timeline's items placed past `floor`, each to
/// `each` in their order, a page of them at a time without their bodies.
fn past_floor(
    library: &fastctx_content::v2::Library,
    scope: &str,
    floor: u64,
    each: &mut dyn FnMut(Vec<u8>) -> Result<(), Failure>,
) -> Result<(), Failure> {
    use fastctx_content::v2::{Kind as _, Timeline};
    // Past every identity at the floor's own place.
    let mut after = Some((floor.to_be_bytes().to_vec(), vec![0xff; 33]));
    loop {
        let page = library.orders(scope, Timeline::STREAM, after.clone(), fastctx_content::v2::PAGE).map_err(failure)?;
        let Some((item, order, _)) = page.last() else { return Ok(()) };
        after = Some((order.clone(), item.clone()));
        for (item, ..) in page {
            each(item)?;
        }
    }
}

/// How many calls whose runs' facts changed are given them again at a time.
const CALLS_A_PAGE: usize = 1024;

/// The version of the facts of the runs `directory` tells of, as they stand.
async fn runs_of(directory: &fastctx_types::layout::ConversationDir) -> Result<i64, Failure> {
    let reading = directory.clone();
    fastctx_platform::task::spawn_blocking(move || crate::run_facts::version(&reading))
        .await
        .map_err(|error| Failure::new(error.to_string()))?
        .map_err(failure)
}

/// What a check found of the record a Claude timeline follows, for its
/// build to go on from the seam only where the build stands where the check
/// found it (codex-e024d399: the hashing is not under the build's hold).
pub(super) struct Checked {
    session: String,
    seam: Seam,
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
    /// Read whole and kept on disk (design §28.51): the events are there.
    kept: Option<Kept>,
}

/// A whole reading kept on disk: a Codex thread's, and what was taken of
/// its events as they came (`codex::follow::Reading`); or a Claude
/// resident's, placed there (`claude::history::kept::Reading`), with what
/// put its events' objects as they were read.
enum Kept {
    Codex { spill: super::spill::Spill, taken: crate::codex::follow::Kept },
    Claude { placed: crate::claude::history::kept::Placed, early: Option<assets::Early> },
}

/// The final events of a reading kept on disk (`finals`), and those of them
/// the host tells again at every reading (`place`'s `unturned`).
struct Finals {
    spill: super::spill::Spill,
    unturned: std::collections::HashSet<Vec<u8>>,
    /// The objects of its events put as they were read.
    early: Option<assets::Early>,
}

/// Where the history is read through now, as its host reads it.
enum Next {
    Claude {
        session: String,
        seam: Box<Seam>,
        kept: Option<String>,
        kept_here: bool,
        earlier: Vec<crate::claude::history::Stamp>,
        prefix_ids: Vec<u64>,
    },
    Codex(CodexSeam),
}

/// Whether `event` is one a Codex reading tells again each time: the
/// thread's title, or what it says of itself.
pub(crate) fn told_again(event: &SessionEvent) -> bool {
    use fastctx_types::session::{Happened, SessionChange};
    matches!(event.happened, Happened::Titled { .. } | Happened::Changed { turn: None, change: SessionChange::Metadata { .. } })
}

/// Whether `event` says Codex put a history in place of the one it had.
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

/// A window's reading of a history, to be placed in the timeline (`place`).
struct Reading {
    history: Vec<SessionEvent>,
    /// What its host tells again at every reading (`Followed::told`).
    host_told: Vec<serde_json::Value>,
    whole: bool,
    since: Timestamp,
    settled: Timestamp,
    /// A Codex thread's: its title and what it says of itself are placed
    /// again where they stand (N1).
    codex: bool,
}

/// Where the last reading left the timeline, as `Following` keeps it — the
/// items it held in its window kept beside the checkpoint `name` under the
/// generation `held` — and the generation this reading keeps those it holds
/// under (`generation`).
struct Stood {
    name: String,
    held: Option<u64>,
    told: Vec<String>,
    floor: u64,
    holding: Option<i64>,
    generation: u64,
}

/// Where a reading leaves the timeline: what it wrote, the order key below
/// which its items are settled, the generation the items it holds in the
/// window are kept under (`Held`), and what the host told again (in hex);
/// of a reading kept on disk, the digests of the record ids before the seam
/// it kept, as the checkpoint keeps them (`Spill::moved_bytes`).
struct Placement {
    wrote: assets::Wrote,
    floor: u64,
    held: u64,
    told: Vec<String>,
    before_seam: Option<Vec<u8>>,
}

/// How many items held in a window are kept beside the checkpoint at once.
const HELD_AT_ONCE: usize = 4096;

/// The items a reading holds in its window kept beside the checkpoint
/// `name` under `generation` (`Library::stage_held`) as they come, a batch
/// at a time: each one's time and order key (design §28.51 ⑤-7). All the
/// next reading asks of them is the highest key of those before its start.
struct Held<'l> {
    library: &'l fastctx_content::v2::Library,
    name: &'l str,
    generation: u64,
    batch: Vec<fastctx_content::v2::HeldItem>,
    fresh: bool,
}

impl<'l> Held<'l> {
    fn new(library: &'l fastctx_content::v2::Library, name: &'l str, generation: u64) -> Self {
        Held { library, name, generation, batch: Vec::new(), fresh: true }
    }

    fn push(&mut self, at: Timestamp, key: u64) -> Result<(), Failure> {
        self.batch.push((at.as_second(), at.subsec_nanosecond(), key));
        if self.batch.len() >= HELD_AT_ONCE {
            self.flush()?;
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<(), Failure> {
        let batch = std::mem::take(&mut self.batch);
        self.library.stage_held(self.name, self.generation, self.fresh, batch).map_err(failure)?;
        self.fresh = false;
        Ok(())
    }

    /// All kept: what an attempt before kept under the generation is gone
    /// even where this one holds none.
    fn finish(mut self) -> Result<u64, Failure> {
        if self.fresh || !self.batch.is_empty() {
            self.flush()?;
        }
        Ok(self.generation)
    }
}

/// The floor a reading from `since` on starts from where the last one
/// `stood`: its floor, and the highest key of the items it held in its
/// window from before `since` that were not told again (R2) — kept beside
/// the checkpoint, told ones left out (`Held`).
fn floor_from(library: &fastctx_content::v2::Library, stood: &Stood, since: Timestamp) -> Result<u64, Failure> {
    let Some(generation) = stood.held else { return Ok(stood.floor) };
    let before = library.held_before(&stood.name, generation, since.as_second(), since.subsec_nanosecond()).map_err(failure)?;
    Ok(before.map_or(stood.floor, |key| key.max(stood.floor)))
}

/// Writes `reading` into the timeline from where the last reading `stood`.
/// Call on a thread that may block; a whole reading after the checkpoint is
/// put back (`reset`).
fn place(
    library: &fastctx_content::v2::Library,
    signer: Arc<dyn fastctx_content::v2::Signer>,
    resident: &ResidentId,
    placed: &assets::Placed,
    stood: &Stood,
    reading: Reading,
) -> Result<Placement, Failure> {
    let Reading { history, host_told, whole, since, settled, codex } = reading;
    // What the last window wrote from before this one began is settled
    // — not what is told again at every reading, whatever its time
    // (R2): its place stays where it is, below the window or in it.
    let floor = floor_from(library, stood, since)?;
    // What the window wrote before, as the library holds it: every
    // item placed past the floor, whatever the checkpoint kept of it
    // (CC12) — a write committed before a stop that kept no
    // checkpoint is among them — and what the host told at the last
    // reading, wherever a whole writing placed it, below the floor
    // too. Each is read again in this window, or gone.
    let before: Vec<(Vec<u8>, Timestamp)> = if whole {
        Vec::new()
    } else {
        let mut items = Vec::new();
        past_floor(library, &placed.scope, floor, &mut |item| {
            items.push(item);
            Ok(())
        })?;
        for identity in stood.told.iter().filter_map(|identity| unhex(identity)) {
            if !items.contains(&identity) {
                items.push(identity);
            }
        }
        items.into_iter().map(|identity| (identity, since)).collect()
    };
    // What the host tells again at every reading stays in the window,
    // whatever its time (a Codex thread's last change, a Claude
    // session's start): never settled below the floor, it goes when
    // a reading no longer tells it (EE01).
    let unturned: std::collections::HashSet<Vec<u8>> = {
        let identities = super::names::identities(&history);
        // What a Codex thread tells again is only of the kinds `told_again`
        // names: no other is compared. A Claude reading tells its source's
        // warnings too.
        history
            .iter()
            .zip(identities)
            .filter(|(event, _)| (!codex || told_again(event)) && serde_json::to_value(event).is_ok_and(|value| host_told.contains(&value)))
            .map(|(_, identity)| identity)
            .collect()
    };
    // A Codex thread's own title and what it says of itself stand at
    // the end of its history: placed again where they stand (N1).
    let again: std::collections::HashSet<Vec<u8>> = if codex { unturned.clone() } else { Default::default() };
    // Read on from where it stood, it is written only in the holding
    // period it stood in.
    let expected = if whole { None } else { stood.holding };
    let span = if whole { Span::Whole } else { Span::From { since, before: &before, floor } };
    let wrote = assets::write(library, signer, resident, placed, &history, span, &again, expected)?;
    // From where the seam stands now, the items before it are settled.
    // Held in the window: what is from where the seam stands on, what
    // is told again at every reading, and an event new to the
    // timeline placed in the window though its time is earlier (DD04)
    // — written past the floor, it goes when the history drops it.
    let held = |identity: &Vec<u8>, at: &Timestamp| *at >= settled || *at < since || unturned.contains(identity);
    let base = if whole { 0 } else { floor };
    let settled_floor = wrote.items.iter().filter(|(identity, at, _)| !held(identity, at)).map(|(_, _, key)| *key).fold(base, u64::max);
    // Kept beside the checkpoint, those told again left out: the next
    // reading leaves them out of its floor (R2).
    let mut open = Held::new(library, &stood.name, stood.generation);
    for (_, at, key) in wrote.items.iter().filter(|(identity, at, _)| held(identity, at) && !unturned.contains(identity)) {
        open.push(*at, *key)?;
    }
    let held = open.finish()?;
    let told = unturned.iter().map(|identity| hex(identity)).collect();
    Ok(Placement { wrote, floor: settled_floor, held, told, before_seam: None })
}

/// The events of a host kept on disk in the order a history shows them
/// (`Spill::hosted`), with their keys for merging (`talk::merging`) and the
/// objects of them put as they were read.
struct Hosted {
    spill: super::spill::Spill,
    keys: crate::talk::merging::HostKeys,
    names: crate::talk::merging::Names,
    early: Option<assets::Early>,
}

impl From<Kept> for Hosted {
    fn from(kept: Kept) -> Self {
        match kept {
            Kept::Codex { spill, taken } => {
                let crate::codex::follow::Kept { keys, names, early, .. } = taken;
                Hosted { spill, keys, names, early }
            }
            Kept::Claude { placed, early } => {
                let crate::claude::history::kept::Placed { spill, keys, names, .. } = placed;
                Hosted { spill, keys, names, early }
            }
        }
    }
}

/// The history a whole reading kept on disk makes of `agent`'s events, as
/// `talk::assemble` makes it of one held in memory, kept there as its final
/// events with their identities; with those of them the host tells again
/// at every reading (`told`, as `place` judges them: `unturned`); where a
/// record stopped short (`frozen`), what each is known by. Call on a thread
/// that may block.
#[allow(clippy::too_many_arguments)]
fn finals(
    directory: Option<&fastctx_types::layout::ConversationDir>,
    hosted: Hosted,
    agent: &str,
    told: &dyn Fn(&SessionEvent) -> bool,
    recorded: crate::talk::Recorded<'_>,
    frozen: bool,
) -> Result<Finals, Failure> {
    let Hosted { spill, keys, mut names, early } = hosted;
    let mut since = std::time::Instant::now();
    let spill = std::cell::RefCell::new(spill);
    // Read back in their order, a batch at a time.
    let mut ahead = super::spill::Ahead::default();
    // Numbered by their places once all are kept (`Spill::number_again`):
    // none of those before is held to tell those alike apart.
    let (mut place, mut told_at) = (0usize, Vec::new());
    crate::talk::assemble_each(
        directory,
        agent,
        recorded,
        keys,
        &mut names,
        &mut |index| ahead.take(index, &mut |from, alone| spill.borrow_mut().hosted_from(from, alone)).map_err(Failure::new),
        &mut |event| {
            let base = super::names::base(&event);
            if told(&event) {
                told_at.push(place);
            }
            place += 1;
            // Its record made now, of the event itself: nothing is read back
            // and made again of what was kept.
            let encoded = serde_json::to_vec(&event).map_err(failure)?;
            let (record, object) = assets::body(&event, &encoded)?;
            let object = object.map(|object| (object, encoded));
            // What a record stopped short knows it by, where one did.
            let known = frozen.then(|| (super::names::frozen(&event), super::names::key(&event).is_some()));
            spill.borrow_mut().push_final(base, event.at, record, object, known).map_err(Failure::new)
        },
    )?;
    let mut spill = spill.into_inner();
    assets::phase("merged and final", &mut since);
    spill.number_again().map_err(Failure::new)?;
    let unturned = spill.identities_at(&told_at).map_err(Failure::new)?.into_iter().collect();
    assets::phase("numbered", &mut since);
    Ok(Finals { spill, unturned, early })
}

/// `place` of a reading whose final events are kept on disk (`finals`):
/// written as `place` writes it, a batch at a time (`assets::write_kept`) —
/// `whole`, or the window from `since` on from where the last reading
/// `stood` (its floor, the items past it and those it told read again into
/// the store; a `codex` thread's told again placed in the window, as
/// `place` places them) — the items it holds in the window kept beside the
/// checkpoint as `stood` says (`Held`). Call on a thread that may block; a
/// whole reading after the checkpoint is put back (`reset`).
#[allow(clippy::too_many_arguments)]
fn place_kept(
    library: &fastctx_content::v2::Library,
    signer: Arc<dyn fastctx_content::v2::Signer>,
    resident: &ResidentId,
    placed: &assets::Placed,
    stood: &Stood,
    finals: Finals,
    whole: bool,
    codex: bool,
    since: Timestamp,
    settled: Timestamp,
) -> Result<Placement, Failure> {
    let Finals { mut spill, unturned, early } = finals;
    // As `place` begins a window: past the floor where the last one left
    // it, reading again what it wrote past it and what the host told.
    let floor = if whole { 0 } else { floor_from(library, stood, since)? };
    if !whole {
        past_floor(library, &placed.scope, floor, &mut |item| spill.push_before(item).map_err(Failure::new))?;
        for identity in stood.told.iter().filter_map(|identity| unhex(identity)) {
            spill.push_before(identity).map_err(Failure::new)?;
        }
    }
    let again: std::collections::HashSet<Vec<u8>> = if codex { unturned.clone() } else { Default::default() };
    let reach = if whole { Reach::Whole } else { Reach::From { since, floor, again: &again, expected: stood.holding } };
    // Held in the window, as `place` holds them, and kept as they come.
    let mut open = Held::new(library, &stood.name, stood.generation);
    let mut held = |identity: &[u8], at: Timestamp, key: u64| -> Result<bool, Failure> {
        let held = at >= settled || at < since || unturned.contains(identity);
        if held && !unturned.contains(identity) {
            open.push(at, key)?;
        }
        Ok(held)
    };
    let wrote = assets::write_kept(library, signer, resident, placed, &mut spill, early.as_ref(), reach, &mut held)?;
    let held = open.finish()?;
    let told = unturned.iter().map(|identity| hex(identity)).collect();
    // Read once the writing let go of what it held.
    let before_seam = Some(spill.moved_bytes().map_err(Failure::new)?);
    Ok(Placement {
        wrote: assets::Wrote { written: wrote.written, gone: wrote.gone, items: Vec::new(), holding: wrote.holding },
        // Of a window, never below where it began (as `place` settles it).
        floor: wrote.floor.max(floor),
        held,
        told,
        before_seam,
    })
}

/// Brings `resident`'s timeline in the asset library to its history, where
/// this machine keeps one.
pub(super) async fn produce(node: &Arc<Node>, resident: &ResidentId, whole: bool) -> Result<Produced, Failure> {
    produce_with(node, resident, whole, None).await
}

/// As `produce`, read on from where it was though its version says nothing
/// changed (`checked`): what a check does once the bytes before the seam are
/// found as they were read (`check`) — the window past it read again, and
/// the history checked as of now where all of it was read.
async fn produce_with(node: &Arc<Node>, resident: &ResidentId, whole: bool, checked: Option<Checked>) -> Result<Produced, Failure> {
    let producing = PRODUCING.hold(resident).await;
    let Some(kept) = node.assets.as_ref() else { return Ok(Produced::default()) };
    let Some((about, _)) = node.residents.about(resident) else { return Ok(Produced::default()) };
    let Some(placed) = assets::placed(node, &about)? else { return Ok(Produced::default()) };
    let library = kept.library.clone();
    let name = assets::checkpoint(resident);
    // With the epoch it is read in: kept again only in it (`publish`).
    let (state, epoch): (Following, u64) = {
        let (reading, name, held) = (library.clone(), name.clone(), producing.clone());
        let (kept, epoch) = fastctx_platform::task::blocking(move || {
            let _held = held;
            reading.checkpoint_in_epoch(&name)
        })
        .await
        .map_err(failure)?;
        (kept.and_then(|bytes| serde_json::from_slice::<Following>(&bytes).ok()).filter(|state| state.form == FORM).unwrap_or_default(), epoch)
    };
    // What this reading holds in its window is kept beside the checkpoint
    // under a generation other than the one the checkpoint names (`Held`).
    let generation = state.held.map_or(1, |held| held.wrapping_add(1));
    // Held anew since — the network left and joined again, the scope
    // revoked and given again — what was made here is of another holding
    // period, whose past may be gone with it: it is made again whole, the
    // history unchanged or not (GG02).
    let holding = {
        let (asking, scope) = (library.clone(), placed.scope.clone());
        fastctx_platform::task::blocking(move || asking.holding(&scope)).await.map_err(failure)?
    };
    let (state, whole) = if state.holding.is_some() && state.holding != holding { (Following::default(), true) } else { (state, whole) };
    // A check's finding stands only for the record and seam it was made of.
    let checked = checked.is_some_and(|checked| {
        state.session.as_deref() == Some(checked.session.as_str())
            && state.seam.identity == checked.seam.identity
            && state.seam.offset == checked.seam.offset
            && state.seam.blocks == checked.seam.blocks
    });
    let version = crate::held_history::version(node, resident).await?;
    if state.version == version && !whole && !checked {
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
        // Described by the hub and every stream of it pulled here once: what
        // orders each is known (MM02).
        let live = fastctx_platform::task::blocking(move || Ok::<_, fastctx_content::Error>(asking.is_live(&scope)? && asking.is_pulled(&scope)?))
            .await
            .map_err(failure)?;
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
    // Read before the history is: a change after it is the next build's.
    let runs_now = runs_of(&directory).await?;
    // As the checkpoint keeps it: an earlier form's digest of them said only
    // that they had not changed, not that every item had them
    // (codex-e024d399), and is not taken for it.
    let runs_known = state.runs_version;
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
        // The World's record past where it was read, kept on disk as it is
        // read (design §28.51 ⑥-1); read whole the first time, or when a cut
        // took turns back, or Codex put its history in place of another.
        let world_spill = || super::spill::Spill::new(kept.spill_dir(), &format!("{}-world", resident.as_str()));
        let read_world = |after: u64, open: Vec<SessionEvent>| {
            let (directory, spill) = (directory.clone(), world_spill());
            fastctx_platform::task::blocking(move || super::world::World::read(spill, &directory, after, &open))
        };
        let mut world = read_world(state.world_from.unwrap_or(0), state.world_open.clone()).await.map_err(Failure::new)?;
        // Only one not heard before: the window reads again what it read. A
        // fact the World tells late, of a time before what is settled, is
        // placed only by reading all again (CC15).
        let (replaced, late) = world.heard_since(state.world_latest, state.settled_at).map_err(Failure::new)?;
        let replaced = agent == "codex" && replaced;
        let mut open = state.world_open.clone();
        let mut whole = whole || replaced || late;
        // A reading of the whole history reads the World's record whole too
        // (R1): what the World alone tells of turns before where it was read
        // through would otherwise be written gone.
        let mut from_start = state.world_from.is_none() || world.cut() || whole;
        if from_start {
            open.clear();
            if state.world_from.is_some() || !state.world_open.is_empty() {
                // The store read let go before it is read again.
                drop(world);
                world = read_world(0, Vec::new()).await.map_err(Failure::new)?;
            }
        }
        let mut world = Arc::new(std::sync::Mutex::new(world));
        let followed = loop {
            let followed = if agent == "claude" {
                // A check reads the record from its start.
                let from = state.session.clone().filter(|_| !whole).map(|session| (session, state.seam.clone(), state.earlier.clone()));
                // Read past a seam, the record ids before it are asked of
                // as they are kept beside the checkpoint (`Keep`), not held.
                let prefix = Arc::new(crate::claude::history::PrefixIds::default());
                let before_seam = if whole { None } else { prefix_generation(&library, resident).await? };
                // Read from its start, kept on disk as it is read (design
                // §28.51 ⑤-5).
                let keep = crate::claude::history::Keep {
                    spill: super::spill::Spill::new(kept.spill_dir(), resident.as_str()),
                    library: library.clone(),
                    seen: before_seam.map(|generation| (prefix_name(resident), generation)),
                };
                crate::claude::history::window(node, resident, &world, from, prefix, Some(keep)).await.map_err(Failure::new)?.map(|window| Followed {
                    events: window.events,
                    told: window.told,
                    whole: window.whole,
                    since: window.since,
                    settled: window.settled,
                    kept: window.held.map(|(placed, early)| Kept::Claude { placed, early }),
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
                // Read whole, kept on disk as it is read (design §28.51).
                let spill = super::spill::Spill::new(kept.spill_dir(), resident.as_str());
                // The turns words were sent into and the names, of all it
                // records; its turns.
                let (sent, names, turns) = {
                    let world = world.clone();
                    fastctx_platform::task::blocking(move || -> Result<_, String> {
                        let mut world = world.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                        world.view(None)?;
                        let (names, sent) = world.name()?;
                        Ok((sent, names, world.turn_spans()))
                    })
                    .await
                    .map_err(Failure::new)?
                };
                let mut reading = crate::codex::follow::Reading::kept(node, resident, sent, names, spill);
                let window = crate::codex::follow::window(node, resident, &about, &turns, from, Some(&mut reading)).await.map_err(Failure::new)?;
                window.map(|window| {
                    let (spill, taken) = reading.finish();
                    let (told, kept) = if window.kept {
                        (taken.told.clone(), Some(Kept::Codex { spill, taken }))
                    } else {
                        (window.events.iter().filter(|event| told_again(event)).cloned().collect(), None)
                    };
                    Followed { told, events: window.events, whole: window.whole, since: window.since, settled: window.settled, next: Next::Codex(window.seam), kept }
                })
            };
            // The host read whole of its own accord (a seam gone, a settled
            // turn changed) while the World's record was read only past
            // where it was: read again, both whole (R1).
            match followed {
                Some(followed) if followed.whole && !from_start => {
                    // What was read let go before all is read again.
                    drop(followed);
                    drop(world);
                    world = Arc::new(std::sync::Mutex::new(read_world(0, Vec::new()).await.map_err(Failure::new)?));
                    open.clear();
                    from_start = true;
                    whole = true;
                }
                followed => break followed,
            }
        };
        followed.map(|followed| (followed, world, open))
    } else {
        None
    };
    let (signer, hub) = (kept.signer(), placed.hub.clone());
    let (wrote, next, unread) = match window {
        Some((window, world, open)) => {
            let (world_from, world_open, latest) = {
                let (world, settled, from) = (world.clone(), window.settled, state.world_from);
                fastctx_platform::task::blocking(move || -> Result<_, String> {
                    let world = world.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                    let (through, open) = world.world_after(open, settled, from)?;
                    Ok((through, open, world.latest()))
                })
                .await
                .map_err(Failure::new)?
            };
            // What the World recorded merged: all of it read whole, else from
            // where the window begins; Codex's reverted turns and requests
            // stay out, as a reading has it.
            let merged = (!window.whole).then_some(window.since);
            // The host's own title and what it says of the thread, told again
            // at every reading (R3: not a title the World recorded).
            let host_told: Vec<serde_json::Value> = window.told.iter().filter_map(|event| serde_json::to_value(event).ok()).collect();
            let (whole, since, settled) = (window.whole, window.since, window.settled);
            let stood = Stood { name: name.clone(), held: state.held, told: state.told.clone(), floor: state.floor, holding: state.holding, generation };
            let Placement { wrote, floor: settled_floor, held, told, before_seam } = match window.kept {
                // Read whole and kept on disk: made into the history there
                // and written a batch at a time (design §28.51).
                Some(kept) => {
                    let finals = {
                        let (folder, held, agent, world) = (directory.clone(), producing.clone(), agent.to_string(), world.clone());
                        fastctx_platform::task::blocking(move || {
                            let _held = held;
                            // As `place` takes what the host tells again: of a
                            // Codex thread, only the kinds `told_again` names.
                            let codex = agent == "codex";
                            let told =
                                |event: &SessionEvent| (!codex || told_again(event)) && serde_json::to_value(event).is_ok_and(|value| host_told.contains(&value));
                            let mut world = world.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                            match &kept {
                                Kept::Codex { taken, .. } => world.view_beside(merged, &|turn| taken.turns.contains(turn)),
                                Kept::Claude { .. } => world.view(merged),
                            }
                            .map_err(Failure::new)?;
                            finals(Some(&folder), kept.into(), &agent, &told, crate::talk::Recorded::Kept(&world), false)
                        })
                        .await?
                    };
                    if whole {
                        reset(&library, &name, epoch, producing.clone()).await?;
                    }
                    let (writing, signer, whose, placed, held, codex) = (library.clone(), signer.clone(), resident.clone(), placed, producing.clone(), agent == "codex");
                    fastctx_platform::task::blocking(move || {
                        let _held = held;
                        place_kept(&writing, signer, &whose, &placed, &stood, finals, whole, codex, since, settled)
                    })
                    .await?
                }
                None => {
                    // Held whole, as the history read is.
                    let recorded = {
                        let world = world.clone();
                        fastctx_platform::task::blocking(move || {
                            let mut world = world.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                            world.view(merged)?;
                            world.events()
                        })
                        .await
                        .map_err(Failure::new)?
                    };
                    let recorded = if agent == "codex" { crate::codex::watch::recorded(recorded, &window.events) } else { recorded };
                    let history = {
                        let (assembling, whose, folder, events, agent) = (node.clone(), resident.clone(), directory.clone(), window.events, agent.to_string());
                        let held = producing.clone();
                        fastctx_platform::task::blocking(move || {
                            let _held = held;
                            crate::talk::assemble(&assembling, &whose, &folder, &agent, recorded, Some(events))
                        })
                        .await?
                    };
                    if whole {
                        reset(&library, &name, epoch, producing.clone()).await?;
                    }
                    let reading = Reading { history, host_told, whole, since, settled, codex: matches!(window.next, Next::Codex(_)) };
                    let (writing, signer, whose, placed, held) = (library.clone(), signer.clone(), resident.clone(), placed, producing.clone());
                    fastctx_platform::task::blocking(move || {
                        let _held = held;
                        place(&writing, signer, &whose, &placed, &stood, reading)
                    })
                    .await?
                }
            };
            // The facts of runs changed since the timeline last had them, of
            // items settled before the window, which is not read again:
            // given again in place, call by call (design §28.53,
            // codex-e024d399 #1); not known call by call, every item before
            // the window that can tell of a run is looked at, a page at a
            // time — the history is not read whole for them.
            let (runs_version, refreshed) = match runs_known {
                _ if whole => (Some(runs_now), 0),
                Some(known) if known == runs_now => (Some(runs_now), 0),
                known => match assets::placed(node, &about)? {
                    Some(placed) => {
                        let (writing, signer, whose, folder, held, holding) =
                            (library.clone(), signer.clone(), resident.clone(), directory.clone(), producing.clone(), wrote.holding);
                        fastctx_platform::task::blocking(move || -> Result<(Option<i64>, usize), Failure> {
                            let _held = held;
                            let attach = |event: &mut SessionEvent| crate::run_facts::attach(&folder, event).map_err(failure);
                            // The calls changed, a page of them at a time.
                            let mut from = known.filter(|known| *known < runs_now);
                            let mut written = 0;
                            while let Some(after) = from {
                                let (_, page) = crate::run_facts::changed_since(&folder, after, CALLS_A_PAGE).map_err(failure)?;
                                let Some((calls, through)) = page else {
                                    written = assets::refresh_every_run(&writing, signer.clone(), &whose, &placed, &attach, settled_floor, Some(holding))?;
                                    return Ok((Some(runs_now), written));
                                };
                                written += assets::refresh_runs(&writing, signer.clone(), &whose, &placed, &attach, &calls, settled_floor, Some(holding))?;
                                from = (through < runs_now && !calls.is_empty()).then_some(through);
                            }
                            // None kept, or kept past the version now (the
                            // facts made anew): every item that can tell of
                            // a run is looked at.
                            if known.is_none_or(|known| known > runs_now) {
                                written = assets::refresh_every_run(&writing, signer, &whose, &placed, &attach, settled_floor, Some(holding))?;
                            }
                            Ok((Some(runs_now), written))
                        })
                        .await?
                    }
                    None => (None, 0),
                },
            };
            // What they wrote is this build's writing too: told on at once.
            let wrote = assets::Wrote { written: wrote.written + refreshed, ..wrote };
            // Read whole, or checked — its prefix as read, the rest read
            // again now (`check`) — the history is checked as of now.
            let verified = if window.whole || checked { Some(Timestamp::now()) } else { state.verified };
            let (next, unread) = match window.next {
                Next::Claude { session, seam, kept: seen, kept_here, earlier, prefix_ids: moved_past } => {
                    keep_prefix_ids(&library, resident, whole, &moved_past, before_seam, producing.clone()).await?;
                    let unread = seen.is_some() && !kept_here && seen != state.kept;
                    let next = Following {
                        form: FORM,
                        // Brought again until the record holds what was kept.
                        version: if unread { String::new() } else { version },
                        holding: Some(wrote.holding),
                        session: Some(session),
                        seam: *seam,
                        held: Some(held),
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
                        runs_version,
                    };
                    (next, unread)
                }
                Next::Codex(seam) => {
                    let next = Following {
                        form: FORM,
                        version,
                        holding: Some(wrote.holding),
                        held: Some(held),
                        told,
                        floor: settled_floor,
                        world_from: Some(world_from),
                        world_open,
                        verified,
                        codex: Some(seam),
                        world_latest: Some(latest),
                        settled_at: Some(settled),
                        runs_version,
                        ..Following::default()
                    };
                    (next, false)
                }
            };
            (wrote, next, unread)
        }
        None => {
            // Read whole into a work store and written from there a batch at
            // a time (design §28.51 ⑤-4), where its host is read still.
            let read_kept = if agent == "claude" && hosted {
                let spill = super::spill::Spill::new(kept.spill_dir(), resident.as_str());
                let world = super::spill::Spill::new(kept.spill_dir(), &format!("{}-world", resident.as_str()));
                let early = assets::Early::new(library.clone(), resident);
                crate::talk::history_kept(node, resident, spill, world, early).await?
            } else {
                None
            };
            let (wrote, floor, unread) = if let Some(read_kept) = read_kept {
                let crate::talk::HistoryKept { hosted: read, world, early } = read_kept;
                let crate::claude::history::HostedKept { placed: history, unread, refused, .. } = read;
                let crate::claude::history::kept::Placed { spill, keys, names, .. } = history;
                reset(&library, &name, epoch, producing.clone()).await?;
                let (writing, signer, whose, folder, held) = (library.clone(), signer.clone(), resident.clone(), directory.clone(), producing.clone());
                let wrote = fastctx_platform::task::blocking(move || {
                    let _held = held;
                    // All the World's record, as it was named (`hosted_kept`).
                    let mut world = world.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                    world.view(None).map_err(Failure::new)?;
                    let mut finals = finals(Some(&folder), Hosted { spill, keys, names, early }, "claude", &|_| false, crate::talk::Recorded::Kept(&world), refused)?;
                    drop(world);
                    // As the history held in memory is written (below), what
                    // records stopped short know their events by kept in the
                    // store.
                    let reach = if refused {
                        Reach::Added
                    } else if unread {
                        Reach::Kept
                    } else {
                        Reach::Whole
                    };
                    assets::write_kept(&writing, signer, &whose, &placed, &mut finals.spill, finals.early.as_ref(), reach, &mut |_, _, _| Ok(false))
                })
                .await?;
                let floor = wrote.floor;
                (assets::Wrote { written: wrote.written, gone: wrote.gone, items: Vec::new(), holding: wrote.holding }, floor, unread)
            } else if let Some(world) = if about.world_managed() && !(hosted && agent == "codex" && about.session.is_some()) {
                // As the World recorded it — its host neither started nor
                // read (asked again as it is read) — kept on disk as it is
                // read and written from there a batch at a time (design
                // §28.51 ⑥-4), as `history_quietly` reads it and the whole is
                // written.
                let world = super::spill::Spill::new(kept.spill_dir(), &format!("{}-world", resident.as_str()));
                crate::talk::recorded_kept(node, resident, world).await?
            } else {
                None
            } {
                reset(&library, &name, epoch, producing.clone()).await?;
                let spill = super::spill::Spill::new(kept.spill_dir(), resident.as_str());
                let early = assets::Early::new(library.clone(), resident);
                let (writing, signer, whose, folder, held) = (library.clone(), signer.clone(), resident.clone(), directory.clone(), producing.clone());
                let wrote = fastctx_platform::task::blocking(move || {
                    let _held = held;
                    let mut world = world;
                    world.view_as_recorded().map_err(Failure::new)?;
                    let mut finals = finals_recorded(&folder, &world, spill, early)?;
                    drop(world);
                    assets::write_kept(&writing, signer, &whose, &placed, &mut finals.spill, finals.early.as_ref(), Reach::Whole, &mut |_, _, _| Ok(false))
                })
                .await?;
                let floor = wrote.floor;
                (assets::Wrote { written: wrote.written, gone: wrote.gone, items: Vec::new(), holding: wrote.holding }, floor, false)
            } else {
                let read = crate::talk::history_quietly(node, resident).await?;
                // A source unreadable now, gone for good or not yet whole, holds
                // up none of the rest: what was read is written, with the note
                // of what could not be, and nothing is taken away until a
                // reading of every source has it gone; it is read again later.
                let unread = read.unread;
                let (refused, frozen) = (read.refused, read.frozen);
                reset(&library, &name, epoch, producing.clone()).await?;
                let wrote = {
                    let (writing, signer, whose, history, held) = (library.clone(), signer.clone(), resident.clone(), read.history, producing.clone());
                    fastctx_platform::task::blocking(move || {
                        let _held = held;
                        // Stopped before a line larger than World reads: what
                        // that record shows is added only where the timeline
                        // never had it (§28.49).
                        let span = if refused {
                            Span::Added(&frozen)
                        } else if unread {
                            Span::Kept
                        } else {
                            Span::Whole
                        };
                        assets::write(&writing, signer, &whose, &placed, &history, span, &Default::default(), None)
                    })
                    .await?
                };
                let floor = wrote.items.iter().map(|(_, _, key)| *key).max().unwrap_or(0);
                (wrote, floor, unread)
            };
            let following = Following {
                form: FORM,
                version: if unread { String::new() } else { version },
                holding: Some(wrote.holding),
                floor,
                verified: if unread { state.verified } else { Some(Timestamp::now()) },
                // What a reading short of a source left stands as it was.
                runs_version: if unread { runs_known } else { Some(runs_now) },
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
    // Only the facts of its runs changing after their items settled (CC15)
    // are not read again from the seam (design §28.53): an append, a long
    // prefix, a write are the follow's own to read.
    let long_changed = followed && next.runs_version.is_none();
    let verified = next.verified;
    let kept_held = next.held;
    let state = serde_json::to_vec(&next).map_err(failure)?;
    publish(&library, &name, state, epoch, producing.clone()).await?;
    // What the checkpoint no longer names goes (what a stop before this
    // leaves goes with the next).
    {
        let (sweeping, name, held) = (library.clone(), name.clone(), producing.clone());
        fastctx_platform::task::blocking(move || {
            let _held = held;
            sweeping.sweep_held(&name, kept_held)
        })
        .await
        .map_err(failure)?;
    }
    if wrote.written + wrote.gone > 0 {
        crate::assets::wake(node, &hub);
    }
    Ok(Produced { written: wrote.written, gone: wrote.gone, unread, long_changed, verified, handing: false })
}

/// The history of the World's record alone (`World::view_as_recorded`)
/// made final in `spill` an event at a time, as `talk::assemble` makes one
/// with no host's (each event's facts of its runs attached) and `finals`
/// keeps it (its record made of the event itself): design §28.51 ⑥-4.
fn finals_recorded(
    directory: &fastctx_types::layout::ConversationDir,
    world: &super::world::World,
    mut spill: super::spill::Spill,
    early: Option<assets::Early>,
) -> Result<Finals, Failure> {
    spill.begin().map_err(Failure::new)?;
    let mut since = std::time::Instant::now();
    world
        .each_event(&mut |mut event| {
            crate::run_facts::attach(directory, &mut event).map_err(|error| crate::record_migration::failure(error).message)?;
            let base = super::names::base(&event);
            let encoded = serde_json::to_vec(&event).map_err(|error| error.to_string())?;
            let (record, object) = assets::body(&event, &encoded).map_err(|failure| failure.message)?;
            spill.push_final(base, event.at, record, object.map(|object| (object, encoded)), None)
        })
        .map_err(Failure::new)?;
    assets::phase("merged and final", &mut since);
    spill.number_again().map_err(Failure::new)?;
    let unturned = spill.identities_at(&[]).map_err(Failure::new)?.into_iter().collect();
    assets::phase("numbered", &mut since);
    Ok(Finals { spill, unturned, early })
}

/// Where the World's record is read through once the events before
/// `settled` are settled, given the events read past where it was, `open`
/// the starts of the turns open there, and `latest`: before the first event
/// of `settled` or later, the starts of the turns not ended by then kept.
#[cfg(test)]
pub(super) fn world_after(
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
            let resting = if late { &LATE_REST_UNTIL } else { &REST_UNTIL };
            let resting = resting.lock().expect("checks' rest").and_then(|until| until.checked_duration_since(Instant::now()));
            if let Some(rest) = resting {
                let until_late = waited.map_or(CHECK_LATE, |waited| CHECK_LATE.saturating_sub(waited));
                let after = if late { rest } else { rest.min(until_late) };
                return Outcome::Again { after: after.max(Duration::from_secs(1)) };
            }
            // Begun: a change from now on asks for another (R4) — not
            // before, so one asked while it rested keeps when it was asked
            // (EE08).
            CHECK_ASKED.lock().expect("checks asked").remove(&resident);
            let began = Instant::now();
            // Ended here before the job's own time is up, so a check that
            // runs too long is tried again as one that fails is (below), not
            // set aside by the jobs.
            let checked = match tokio::time::timeout(WITHIN.saturating_sub(CHECK_ENDS_BEFORE), check(&node, &resident)).await {
                Ok(checked) => checked,
                Err(_) => Err(failure(format!("the check ran past {} s", WITHIN.saturating_sub(CHECK_ENDS_BEFORE).as_secs()))),
            };
            let took = began.elapsed();
            // Whatever came of it — a reading short of a source, a failure,
            // one that ran too long — the rest follows it (codex-e024d399 #6).
            {
                let now = Instant::now();
                for (resting, rest) in [(&REST_UNTIL, took * (VERIFY_SHARE - 1)), (&LATE_REST_UNTIL, took)] {
                    let mut resting = resting.lock().expect("checks' rest");
                    let until = now.checked_add(rest);
                    *resting = (*resting).max(until);
                }
            }
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
                    if late {
                        node.log.line(format_args!("{resident}'s check began late: checks are behind and take half a core"));
                    }
                    Outcome::Done
                }
                // Never set aside: a check asked for is done — what a Codex
                // turn read a page at a time leaves to it among it — tried
                // again an hour on, and so on until it is.
                Err(failure) if taken.attempt >= TRIES => {
                    node.log.line(format_args!(
                        "{resident}'s check failed {TRIES} times: {}; tried again in an hour",
                        failure.message
                    ));
                    Outcome::Again { after: LONG_WITHIN }
                }
                Err(failure) => Outcome::Failed(failure.message),
            }
        })
    };
    jobs.register(Kind { name: VERIFY, at_once: 1, within: WITHIN, tries: TRIES }, Arc::new(run));
}

/// Checks `resident`'s history (§10, design §28.53). A Claude record
/// followed past a seam, the only record its history is read from, whose
/// session started no agent (codex-e024d399 #2, #3: what an earlier record
/// or an agent's meta file beside it holds is not what the hashes below
/// tell of), has the bytes before the seam hashed whole against what was
/// read: found as they were, it is
/// read on from the seam (the window past it read again), not read whole.
/// Found otherwise, or any other history — a Codex thread, whose old turns
/// only a listing tells; one read as the World recorded it — is read whole.
async fn check(node: &Arc<Node>, resident: &ResidentId) -> Result<Produced, Failure> {
    if let Some((path, state)) = followed_record(node, resident).await? {
        let beside = {
            let path = path.clone();
            fastctx_platform::task::spawn_blocking(move || fastctx_hosts::claude::subagents::beside(&path))
                .await
                .map_err(|error| Failure::new(error.to_string()))?
        };
        // The facts of its runs are the build's own to bring (`produce_with`),
        // a page at a time where not known call by call.
        let alone = state.earlier.is_empty() && state.seam.agents.is_empty() && beside.is_empty();
        if alone
            && let Some(session) = state.session
        {
            // Given up as soon as the check is: one that ran too long is not
            // still hashing beside the next (codex-e024d399 #5).
            let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
            struct Stop(Arc<std::sync::atomic::AtomicBool>);
            impl Drop for Stop {
                fn drop(&mut self) {
                    self.0.store(true, std::sync::atomic::Ordering::Relaxed);
                }
            }
            let _stop = Stop(stop.clone());
            let seam = state.seam;
            let hashing = seam.clone();
            let held = fastctx_platform::task::blocking(move || {
                fastctx_hosts::claude::transcript::follow::prefix_unchanged(&path, &hashing, &|| stop.load(std::sync::atomic::Ordering::Relaxed))
            })
            .await;
            match held {
                Ok(true) => return produce_with(node, resident, false, Some(Checked { session, seam })).await,
                Ok(false) => node.log.line(format_args!("{resident}'s record changed before where it was read through: read whole")),
                Err(error) => node.log.line(format_args!("{resident}'s record could not be checked ({error}): read whole")),
            }
        }
    }
    produce(node, resident, true).await
}

/// The record a Claude resident's timeline follows past a seam, and where
/// its timeline stands, as its checkpoint keeps it: none where it follows
/// none (another host, read whole, read as the World recorded it).
async fn followed_record(node: &Arc<Node>, resident: &ResidentId) -> Result<Option<(std::path::PathBuf, Following)>, Failure> {
    let Some(kept) = node.assets.as_ref() else { return Ok(None) };
    let Some((about, _)) = node.residents.about(resident) else { return Ok(None) };
    if about.host.agent.as_str() != "claude" {
        return Ok(None);
    }
    let (reading, name) = (kept.library.clone(), assets::checkpoint(resident));
    let state = fastctx_platform::task::blocking(move || reading.checkpoint(&name)).await.map_err(failure)?;
    let Some(state) = state.and_then(|bytes| serde_json::from_slice::<Following>(&bytes).ok()).filter(|state| state.form == FORM) else {
        return Ok(None);
    };
    let (Some(session), Some(_)) = (&state.session, state.seam.identity.as_ref()) else { return Ok(None) };
    let directory = fastctx_hosts::claude::exchange::sessions(node.world.path()).join(session);
    let path = fastctx_platform::task::blocking(move || fastctx_hosts::claude::exchange::transcript(&directory)).await.ok().flatten();
    Ok(path.map(|path| (path, state)))
}

/// Asks for `resident`'s history to be checked whole, begun within the
/// first half of the next hour so it is done within it (R5); asked once
/// while one waits (a check begun frees the asking, R4) — and one waiting
/// keeps when it was asked for and when it is due, however often it is
/// asked again, each start of the node asking (codex-e024d399 #4).
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
            match database.write_relaxed(bytes, move |db| fastctx_jobs::submit_unless_waiting(db, &asked, pause)).await {
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
    // The files attached to it, their originals as objects (§28.16).
    match super::attachments::produce(node, resident, &PRODUCING).await {
        Ok(0) => {}
        Ok(written) => node.log.line(format_args!("{resident}'s attachments: {written} written")),
        Err(error) => {
            beside_failed = true;
            node.log.line(format_args!("{resident}'s attachments: {error}"));
        }
    }
    // Its timers and its evidence folders (§28.22).
    match super::timers::produce(node, resident, &PRODUCING).await {
        Ok(0) => {}
        Ok(written) => node.log.line(format_args!("{resident}'s timers: {written} written")),
        Err(error) => {
            beside_failed = true;
            node.log.line(format_args!("{resident}'s timers: {error}"));
        }
    }
    // Its input-box draft, as the composer here has it (§28.24).
    match super::draft::produce(node, resident, &PRODUCING).await {
        Ok(0) => {}
        Ok(written) => node.log.line(format_args!("{resident}'s draft: {written} written")),
        Err(error) => {
            beside_failed = true;
            node.log.line(format_args!("{resident}'s draft: {error}"));
        }
    }
    match super::artifacts::produce(node, resident, &PRODUCING).await {
        Ok(0) => {}
        Ok(written) => node.log.line(format_args!("{resident}'s evidence folders: {written} written")),
        Err(error) => {
            beside_failed = true;
            node.log.line(format_args!("{resident}'s evidence folders: {error}"));
        }
    }
    match super::file_changes::produce(node, resident, &PRODUCING).await {
        Ok(0) => {}
        Ok(written) => node.log.line(format_args!("{resident}'s file changes: {written} written")),
        Err(error) => {
            beside_failed = true;
            node.log.line(format_args!("{resident}'s file changes: {error}"));
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

/// A Codex thread's timeline written reading after reading as `produce`
/// writes it, for fastctx-bench (`codex-advance`, `codex-relist`, §28.50):
/// the same placing (`place`) and checkpoint, without a node — no World
/// record, deliveries or run facts placed among the thread's events, the
/// scope held already.
#[cfg(feature = "bench")]
pub mod bench {
    use super::*;

    /// What the bench's sampler saw resident now: the most of each part
    /// of a reading is kept (`Follower::phase_peaks`).
    pub fn note_resident(now: u64) {
        assets::PHASE_PEAK.fetch_max(now, std::sync::atomic::Ordering::Relaxed);
    }

    pub struct Follower {
        library: fastctx_content::v2::Library,
        signer: Arc<dyn fastctx_content::v2::Signer>,
        resident: ResidentId,
        placed: assets::Placed,
    }

    impl Follower {
        /// Follows into `scope` of `library`, held already.
        pub fn new(library: fastctx_content::v2::Library, signer: Arc<dyn fastctx_content::v2::Signer>, scope: &str) -> Result<Self, String> {
            let hub = fastctx_types::access::HubId::from_bytes(*b"benchhub");
            let network = library.join(&hub.to_string(), &signer.device(), false).map_err(|error| error.to_string())?;
            let readers = fastctx_types::assets::Readers::Conversation { openers: Vec::new(), born_on: None };
            let proposal = crate::assets::Proposal { hub: hub.to_string(), readers, classes: Vec::new() };
            let placed = assets::Placed { scope: scope.to_string(), hub, proposal, network };
            let resident: ResidentId = "codex-be0c".parse().map_err(|error| format!("{error}"))?;
            Ok(Self { library, signer, resident, placed })
        }

        /// Places a reading of the thread — `events`, read whole or from
        /// `since`, settled before `settled`, to `seam` — from where the
        /// checkpoint says the last one stood, and keeps the checkpoint, as
        /// `produce` does: how many items it wrote and how many went.
        pub fn read(
            &self,
            events: Vec<SessionEvent>,
            whole: bool,
            since: Option<Timestamp>,
            settled: Option<Timestamp>,
            seam: CodexSeam,
        ) -> Result<(usize, usize), String> {
            // As `codex::follow::window` and `produce` take it.
            let since = if whole { Timestamp::MIN } else { since.unwrap_or(Timestamp::MAX) };
            let settled = settled.unwrap_or(since);
            let host_told = events.iter().filter(|event| told_again(event)).filter_map(|event| serde_json::to_value(event).ok()).collect();
            assets::begin_phases();
            let began = std::time::Instant::now();
            let history = crate::talk::merged(Vec::new(), Some(events), "codex", &fastctx_types::session::attachments::NoReceipts);
            assets::PHASES.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(("final events", began.elapsed().as_secs_f64()));
            let name = assets::checkpoint(&self.resident);
            // What `produce` reads first: where the timeline stands, the
            // scope's holding period, and who orders its stream.
            let failed = |error: fastctx_content::Error| error.to_string();
            let kept = self.library.checkpoint(&name).map_err(failed)?;
            let state = kept.and_then(|bytes| serde_json::from_slice::<Following>(&bytes).ok()).filter(|state| state.form == FORM).unwrap_or_default();
            self.library.holding(&self.placed.scope).map_err(failed)?;
            self.library.orderer(&self.placed.scope, <fastctx_content::v2::Timeline as fastctx_content::v2::Kind>::STREAM).map_err(failed)?;
            if whole {
                let state = serde_json::to_vec(&Following { form: FORM, ..Following::default() }).map_err(|error| error.to_string())?;
                self.library.keep_checkpoint(&name, state).map_err(|error| error.to_string())?;
            }
            let generation = state.held.map_or(1, |held| held.wrapping_add(1));
            let stood = if whole {
                Stood { name: name.clone(), held: None, told: Vec::new(), floor: 0, holding: None, generation }
            } else {
                Stood { name: name.clone(), held: state.held, told: state.told, floor: state.floor, holding: state.holding, generation }
            };
            let reading = Reading { history, host_told, whole, since, settled, codex: true };
            let Placement { wrote, floor, held, told, .. } =
                place(&self.library, self.signer.clone(), &self.resident, &self.placed, &stood, reading).map_err(|failure| failure.message)?;
            let next = Following {
                form: FORM,
                version: "bench".into(),
                holding: Some(wrote.holding),
                held: Some(held),
                told,
                floor,
                world_from: Some(0),
                verified: if whole { Some(Timestamp::now()) } else { state.verified },
                codex: Some(seam),
                world_latest: Some(0),
                ..Following::default()
            };
            let next = serde_json::to_vec(&next).map_err(|error| error.to_string())?;
            self.library.keep_checkpoint(&name, next).map_err(failed)?;
            Ok((wrote.written, wrote.gone))
        }

        /// Places the Claude record at `path` as `produce` places a Claude
        /// resident's history with no window to follow: one session begun
        /// at `began`, no World turns; read into a work store under `root`
        /// and written from there a batch at a time (design §28.51 ⑤-4),
        /// or, with no `root`, held whole and written. How many items it
        /// wrote.
        pub fn claude_place(&self, path: &std::path::Path, began: Timestamp, root: Option<std::path::PathBuf>) -> Result<usize, String> {
            let failed = |error: fastctx_content::Error| error.to_string();
            self.library.holding(&self.placed.scope).map_err(failed)?;
            self.library.orderer(&self.placed.scope, <fastctx_content::v2::Timeline as fastctx_content::v2::Kind>::STREAM).map_err(failed)?;
            let receipts = fastctx_types::session::attachments::NoReceipts;
            let Some(root) = root else {
                let read = fastctx_hosts::claude::transcript::read_continuing(path, &[], &Default::default()).map_err(|error| error.to_string())?;
                let mut events = Vec::new();
                if let Some(title) = read.title.clone() {
                    events.push(SessionEvent { at: began, happened: fastctx_types::session::Happened::Titled { title } });
                }
                events.extend(read.events);
                if let Some(text) = crate::claude::history::problem_note("bench", &read.problems, read.problems_omitted) {
                    events.push(crate::claude::history::note(began, text));
                }
                let placed = crate::claude::history::place(events, &[]);
                let history = crate::talk::merged(Vec::new(), Some(placed), "claude", &receipts);
                let wrote = assets::write(&self.library, self.signer.clone(), &self.resident, &self.placed, &history, Span::Whole, &Default::default(), None)
                    .map_err(|failure| failure.message)?;
                return Ok(wrote.written);
            };
            assets::begin_phases();
            let mut since = std::time::Instant::now();
            let mut reading = crate::claude::history::kept::Reading::new(super::super::spill::Spill::new(root, self.resident.as_str()))?;
            let read = reading.read(path, &[], &mut |_| {}).map_err(|error| error.to_string())?;
            let refused = read.refused.is_some();
            reading.accept("bench", began, &read)?;
            assets::phase("read", &mut since);
            let early = assets::Early::new(self.library.clone(), &self.resident);
            let placed = reading.place(crate::claude::history::kept::Placing::Held { recorded: &[], merged: &[] }, &mut |_| Ok(()), &receipts, early.as_ref(), &|_| false)?;
            assets::phase("placed", &mut since);
            let crate::claude::history::kept::Placed { spill, keys, names, .. } = placed;
            let mut finals = finals(None, Hosted { spill, keys, names, early }, "claude", &|_| false, crate::talk::Recorded::Held(Vec::new()), refused).map_err(|failure| failure.message)?;
            // Stopped short, added only where the timeline has none of it.
            let reach = if refused { Reach::Added } else { Reach::Whole };
            let wrote = assets::write_kept(&self.library, self.signer.clone(), &self.resident, &self.placed, &mut finals.spill, finals.early.as_ref(), reach, &mut |_, _, _| Ok(false))
                .map_err(|failure| failure.message)?;
            Ok(wrote.written)
        }

        /// Follows the Claude record at `path`, of one session begun at
        /// `began`, from its start as the node follows a resident's record
        /// read whole (`claude::history::window`, the World's record empty):
        /// held in memory (`follow::tail`, `history::place`, `talk::merged`,
        /// `place`), or, with `root`, read into a work store there and
        /// written from it (§28.51 ⑤-5: `kept::Reading::tail`, `finals`,
        /// `place_kept`). How many items were written, and how many record
        /// ids the seam moved past.
        pub fn claude_follow(
            &self,
            path: &std::path::Path,
            began: Timestamp,
            root: Option<std::path::PathBuf>,
            world: &mut dyn FnMut() -> Box<dyn Iterator<Item = SessionEvent>>,
        ) -> Result<(usize, usize), String> {
            use fastctx_hosts::claude::transcript::follow;
            let failed = |error: fastctx_content::Error| error.to_string();
            self.library.holding(&self.placed.scope).map_err(failed)?;
            self.library.orderer(&self.placed.scope, <fastctx_content::v2::Timeline as fastctx_content::v2::Kind>::STREAM).map_err(failed)?;
            let receipts = fastctx_types::session::attachments::NoReceipts;
            let stood = Stood { name: assets::checkpoint(&self.resident), held: None, told: Vec::new(), floor: 0, holding: None, generation: 1 };
            let is_told = |event: &SessionEvent, title: Option<&str>, problem: Option<&str>| match &event.happened {
                fastctx_types::session::Happened::Titled { title: told } => event.at == began && title == Some(told.as_str()),
                fastctx_types::session::Happened::Health { health, .. } => event.at == began && problem == Some(health.text.as_str()),
                _ => false,
            };
            let Some(root) = root else {
                let tail = follow::tail(path, &follow::Seam::default(), &Default::default(), &|_| true, false, &|_| false)
                    .map_err(|error| error.to_string())?
                    .map_err(|why| why.to_string())?;
                let read = &tail.transcript;
                let mut events = Vec::new();
                if let Some(title) = read.title.clone() {
                    events.push(SessionEvent { at: began, happened: fastctx_types::session::Happened::Titled { title } });
                }
                events.extend(read.events.iter().cloned());
                let problem = crate::claude::history::problem_note("bench", &read.problems, read.problems_omitted);
                if let Some(text) = &problem {
                    events.push(crate::claude::history::note(began, text.clone()));
                }
                let moved: Vec<u64> = tail.prefix_ids.iter().map(|id| crate::claude::history::PrefixIds::digest(id)).collect();
                let recorded: Vec<SessionEvent> = world().collect();
                let placed = crate::claude::history::place(events, &recorded);
                let host_told: Vec<serde_json::Value> =
                    placed.iter().filter(|event| is_told(event, read.title.as_deref(), problem.as_deref())).filter_map(|event| serde_json::to_value(event).ok()).collect();
                let history = crate::talk::merged(recorded, Some(placed), "claude", &receipts);
                let settled = tail.seam_at.unwrap_or(Timestamp::MIN);
                let reading = Reading { history, host_told, whole: true, since: Timestamp::MIN, settled, codex: false };
                let placement = place(&self.library, self.signer.clone(), &self.resident, &self.placed, &stood, reading).map_err(|failure| failure.message)?;
                return Ok((placement.wrote.written, moved.len()));
            };
            assets::begin_phases();
            let mut since = std::time::Instant::now();
            let mut reading = crate::claude::history::kept::Reading::new(super::super::spill::Spill::new(root.clone(), self.resident.as_str()))?;
            let tail = reading
                .tail(path, &follow::Seam::default(), &|_| true, false, &|_| Ok(Default::default()), &[], &crate::claude::history::PrefixIds::digest, &mut |_| {})
                .map_err(|error| error.to_string())?
                .map_err(|why| why.to_string())?;
            let read = &tail.transcript;
            let problem = crate::claude::history::problem_note("bench", &read.problems, read.problems_omitted);
            reading.accept("bench", began, read)?;
            assets::phase("read", &mut since);
            let early = assets::Early::new(self.library.clone(), &self.resident);
            let title = read.title.clone();
            let told = |event: &SessionEvent| is_told(event, title.as_deref(), problem.as_deref());
            // The World's record as `produce` reads it whole: kept on disk
            // as it is read (§28.51 ⑥-1), named, its keys kept beside it.
            let events = world().enumerate().map(|(at, event)| (at as u64 + 1, event));
            let mut world = super::super::world::World::of(super::super::spill::Spill::new(root.clone(), "bench-world"), events, 0, 0, &[])?;
            world.view(None)?;
            let (names, sent) = world.name()?;
            assets::phase("world read", &mut since);
            let failed = std::cell::RefCell::new(None);
            let heard = |turn: &str, text: &str| {
                world.heard_in(turn, text).unwrap_or_else(|error| {
                    failed.borrow_mut().get_or_insert(error);
                    false
                })
            };
            let kept_world = crate::claude::history::kept::Placing::Kept { turns: world.turns(&heard), sent, names };
            let placed = reading.place(kept_world, &mut |_| Ok(()), &receipts, early.as_ref(), &told)?;
            if let Some(error) = failed.into_inner() {
                return Err(error);
            }
            assets::phase("placed", &mut since);
            let host_told: Vec<serde_json::Value> = placed.told.iter().filter_map(|event| serde_json::to_value(event).ok()).collect();
            let told = |event: &SessionEvent| serde_json::to_value(event).is_ok_and(|value| host_told.contains(&value));
            let finals = finals(None, Kept::Claude { placed, early }.into(), "claude", &told, crate::talk::Recorded::Kept(&world), false).map_err(|failure| failure.message)?;
            drop(world);
            let settled = tail.seam_at.unwrap_or(Timestamp::MIN);
            let placement =
                place_kept(&self.library, self.signer.clone(), &self.resident, &self.placed, &stood, finals, true, false, Timestamp::MIN, settled).map_err(|failure| failure.message)?;
            Ok((placement.wrote.written, placement.before_seam.map_or(0, |bytes| bytes.len() / 8)))
        }

        /// Writes the history of the World's record alone, `events`, as
        /// `produce` writes one whose host is not read (§28.51 ⑥-4): kept
        /// in a store under `root` as it is read, ordered there, made final
        /// and written a batch at a time. How many items it wrote.
        pub fn world_alone(&self, events: Box<dyn Iterator<Item = SessionEvent>>, root: std::path::PathBuf) -> Result<usize, String> {
            let failed = |error: fastctx_content::Error| error.to_string();
            self.library.holding(&self.placed.scope).map_err(failed)?;
            self.library.orderer(&self.placed.scope, <fastctx_content::v2::Timeline as fastctx_content::v2::Kind>::STREAM).map_err(failed)?;
            assets::begin_phases();
            let mut since = std::time::Instant::now();
            let events = events.enumerate().map(|(at, event)| (at as u64 + 1, event));
            let mut world = super::super::world::World::of_recorded(super::super::spill::Spill::new(root.clone(), "bench-world"), events)?;
            world.view_as_recorded()?;
            assets::phase("world read", &mut since);
            let directory = fastctx_types::layout::ConversationDir::at(&root.join("conversation"));
            let early = assets::Early::new(self.library.clone(), &self.resident);
            let mut finals = finals_recorded(&directory, &world, super::super::spill::Spill::new(root, self.resident.as_str()), early).map_err(|failure| failure.message)?;
            drop(world);
            let wrote = assets::write_kept(&self.library, self.signer.clone(), &self.resident, &self.placed, &mut finals.spill, finals.early.as_ref(), Reach::Whole, &mut |_, _, _| Ok(false))
                .map_err(|failure| failure.message)?;
            Ok(wrote.written)
        }

        /// Follows the Claude record at `path`, of one session begun at
        /// `began`, twice, read into a work store under `root` (§28.51
        /// ⑤-7): from its start, then — after `between`, which grows it —
        /// on from where the first left its seam, written from where the
        /// first left the timeline (its floor, the items it held in its
        /// window kept beside the checkpoint, what it told). How many items
        /// each wrote, and whether the second read a window.
        pub fn claude_follow_twice(
            &self,
            path: &std::path::Path,
            began: Timestamp,
            root: std::path::PathBuf,
            between: &mut dyn FnMut() -> Result<(), String>,
        ) -> Result<(usize, usize, bool), String> {
            use fastctx_hosts::claude::transcript::follow;
            let failed = |error: fastctx_content::Error| error.to_string();
            self.library.holding(&self.placed.scope).map_err(failed)?;
            let receipts = fastctx_types::session::attachments::NoReceipts;
            let name = assets::checkpoint(&self.resident);
            let mut stood = Stood { name: name.clone(), held: None, told: Vec::new(), floor: 0, holding: None, generation: 1 };
            let (mut seam, mut prefix, mut written) = (follow::Seam::default(), crate::claude::history::PrefixIds::default(), Vec::new());
            let mut window = false;
            for round in 0..2 {
                if round == 1 {
                    between()?;
                }
                let whole = seam.identity.is_none();
                window = !whole;
                let mut reading = crate::claude::history::kept::Reading::new(super::super::spill::Spill::new(root.clone(), self.resident.as_str()))?;
                let seen = |uuids: &[String]| -> std::io::Result<std::collections::HashSet<String>> { Ok(uuids.iter().filter(|uuid| prefix.holds(uuid)).cloned().collect()) };
                let tail = reading
                    .tail(path, &seam, &|_| true, false, &seen, &[], &crate::claude::history::PrefixIds::digest, &mut |_| {})
                    .map_err(|error| error.to_string())?
                    .map_err(|why| why.to_string())?;
                let read = &tail.transcript;
                let problem = crate::claude::history::problem_note("bench", &read.problems, read.problems_omitted);
                reading.accept("bench", began, read)?;
                let early = assets::Early::new(self.library.clone(), &self.resident);
                let title = read.title.clone();
                let told = |event: &SessionEvent| match &event.happened {
                    fastctx_types::session::Happened::Titled { title: told } => event.at == began && title.as_deref() == Some(told.as_str()),
                    fastctx_types::session::Happened::Health { health, .. } => event.at == began && problem.as_deref() == Some(health.text.as_str()),
                    _ => false,
                };
                let placed = reading.place(crate::claude::history::kept::Placing::Held { recorded: &[], merged: &[] }, &mut |_| Ok(()), &receipts, early.as_ref(), &told)?;
                let host_told: Vec<serde_json::Value> = placed.told.iter().filter_map(|event| serde_json::to_value(event).ok()).collect();
                let told = |event: &SessionEvent| serde_json::to_value(event).is_ok_and(|value| host_told.contains(&value));
                let finals = finals(None, Kept::Claude { placed, early }.into(), "claude", &told, crate::talk::Recorded::Held(Vec::new()), false).map_err(|failure| failure.message)?;
                let since = if whole { Timestamp::MIN } else { tail.since.unwrap_or(Timestamp::MAX) };
                let settled = tail.seam_at.unwrap_or(since);
                let placement =
                    place_kept(&self.library, self.signer.clone(), &self.resident, &self.placed, &stood, finals, whole, false, since, settled).map_err(|failure| failure.message)?;
                written.push(placement.wrote.written);
                let moved: Vec<u64> =
                    placement.before_seam.unwrap_or_default().chunks_exact(8).map(|chunk| u64::from_le_bytes(chunk.try_into().expect("eight bytes"))).collect();
                prefix.0.extend(moved);
                prefix.0.sort_unstable();
                prefix.0.dedup();
                stood = Stood {
                    name: name.clone(),
                    held: Some(placement.held),
                    told: placement.told,
                    floor: placement.floor,
                    holding: Some(placement.wrote.holding),
                    generation: placement.held + 1,
                };
                seam = tail.seam.clone().unwrap_or(seam);
            }
            Ok((written[0], written[1], window))
        }

        /// Reads the Claude record at `path` as a session's history with
        /// its rows kept in a work store under `root` (`spill::Lines`,
        /// §28.51 ⑤-3), the events made of it kept there as they are made:
        /// how many events, and problems.
        pub fn claude_kept(root: std::path::PathBuf, path: &std::path::Path) -> Result<(usize, usize), String> {
            use super::super::spill::{Lines, Spill, pack_event};
            let mut spill = Spill::new(root, "claude-bench");
            spill.begin()?;
            let cell = std::cell::RefCell::new(spill);
            let mut events = 0;
            let read = fastctx_hosts::claude::transcript::read_history_kept(path, &[], &|_| Ok(false), &mut Lines(&cell), &mut |event| {
                cell.borrow_mut().push_hosted(pack_event(&event).map_err(std::io::Error::other)?).map_err(std::io::Error::other)?;
                events += 1;
                Ok(())
            })
            .map_err(|error| error.to_string())?;
            Ok((events, read.problems.len()))
        }

        /// Somewhere to keep a whole reading of the thread on disk as it is
        /// read (`drill::tail`), under `root`.
        pub fn keeping(&self, root: std::path::PathBuf) -> Spilled {
            // With its objects put as they are read, unless `BENCH_NO_EARLY`.
            let library = std::env::var_os("BENCH_NO_EARLY").is_none().then(|| (self.library.clone(), self.resident.clone()));
            Spilled(crate::codex::follow::Reading::bare(super::super::spill::Spill::new(root, self.resident.as_str()), library))
        }

        /// As `read` of a whole reading, of one kept on disk (`keeping`)
        /// rather than held in memory, as `produce` places it.
        pub fn read_kept(&self, keeping: Spilled, settled: Option<Timestamp>, seam: CodexSeam) -> Result<(usize, usize), String> {
            let (spill, taken) = keeping.0.finish();
            if !spill.begun() {
                return Err("the reading was not kept".into());
            }
            let since = Timestamp::MIN;
            let settled = settled.unwrap_or(since);
            let host_told: Vec<serde_json::Value> = taken.told.iter().filter_map(|event| serde_json::to_value(event).ok()).collect();
            let name = assets::checkpoint(&self.resident);
            let failed = |error: fastctx_content::Error| error.to_string();
            assets::begin_phases();
            let began = std::time::Instant::now();
            let told = |event: &SessionEvent| told_again(event) && serde_json::to_value(event).is_ok_and(|value| host_told.contains(&value));
            let finals = finals(None, Kept::Codex { spill, taken }.into(), "codex", &told, crate::talk::Recorded::Held(Vec::new()), false).map_err(|failure| failure.message)?;
            assets::PHASES.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(("final events", began.elapsed().as_secs_f64()));
            let state = serde_json::to_vec(&Following { form: FORM, ..Following::default() }).map_err(|error| error.to_string())?;
            self.library.keep_checkpoint(&name, state).map_err(failed)?;
            let stood = Stood { name: name.clone(), held: None, told: Vec::new(), floor: 0, holding: None, generation: 1 };
            let Placement { wrote, floor, held, told, .. } =
                place_kept(&self.library, self.signer.clone(), &self.resident, &self.placed, &stood, finals, true, true, since, settled).map_err(|failure| failure.message)?;
            let next = Following {
                form: FORM,
                version: "bench".into(),
                holding: Some(wrote.holding),
                held: Some(held),
                told,
                floor,
                world_from: Some(0),
                verified: Some(Timestamp::now()),
                codex: Some(seam),
                world_latest: Some(0),
                ..Following::default()
            };
            let next = serde_json::to_vec(&next).map_err(|error| error.to_string())?;
            self.library.keep_checkpoint(&name, next).map_err(failed)?;
            Ok((wrote.written, wrote.gone))
        }

        /// How long each part of the last `read_kept` took, in seconds.
        pub fn phases(&self) -> Vec<(&'static str, f64)> {
            assets::PHASES.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
        }

        /// The most resident memory the sampler saw in each part of the
        /// last reading (`note_resident`), in bytes.
        pub fn phase_peaks(&self) -> Vec<(&'static str, u64)> {
            assets::PEAKS.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
        }

        /// Every live item of the timeline, in order: identity, order key,
        /// body and objects.
        pub fn items(&self) -> Result<Vec<Item>, String> {
            let mut items = Vec::new();
            let mut after = None;
            loop {
                let page = self.library.page(&self.placed.scope, <fastctx_content::v2::Timeline as fastctx_content::v2::Kind>::STREAM, after.clone(), fastctx_content::v2::PAGE).map_err(|error| error.to_string())?;
                let Some(last) = page.last() else { break };
                after = Some((last.order.clone(), last.item.clone()));
                let full = page.len() == fastctx_content::v2::PAGE;
                items.extend(page.into_iter().map(|entry| (entry.item, entry.order, entry.body, entry.objects)));
                if !full {
                    break;
                }
            }
            Ok(items)
        }

        /// The checkpoint as it stands, as JSON.
        pub fn checkpoint(&self) -> Result<serde_json::Value, String> {
            let kept = self.library.checkpoint(&assets::checkpoint(&self.resident)).map_err(|error| error.to_string())?;
            let mut state = kept.map(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes)).transpose().map_err(|error| error.to_string())?.unwrap_or_default();
            // When it was checked is the moment it ran, not what it read.
            if let Some(state) = state.as_object_mut() {
                state.remove("verified");
            }
            Ok(state)
        }
    }

    /// An item of the timeline: identity, order key, body and objects.
    pub type Item = (Vec<u8>, Vec<u8>, Vec<u8>, Vec<fastctx_content::ObjectRef>);

    /// A whole reading kept on disk as it is read (`Follower::keeping`).
    pub struct Spilled(crate::codex::follow::Reading);

    impl fastctx_hosts::codex::follow::Keeping for Spilled {
        fn begin(&mut self) -> Result<(), String> {
            self.0.begin()
        }
        fn time(&mut self, turn: &str, id: &str, timing: fastctx_hosts::codex::follow::Timing) -> Result<(), String> {
            self.0.time(turn, id, timing)
        }
        fn timing(&mut self, turn: &str, id: &str) -> Result<Option<fastctx_hosts::codex::follow::Timing>, String> {
            self.0.timing(turn, id)
        }
        fn item(&mut self, turn: &str, entry: serde_json::Value) -> Result<(), String> {
            self.0.item(turn, entry)
        }
        fn items_of(&mut self, turn: &str, from: usize, limit: usize) -> Result<Vec<serde_json::Value>, String> {
            self.0.items_of(turn, from, limit)
        }
        fn entry(&mut self, entry: serde_json::Value) -> Result<(), String> {
            self.0.entry(entry)
        }
        fn entries(&mut self, from: usize, limit: usize) -> Result<Vec<serde_json::Value>, String> {
            self.0.entries(from, limit)
        }
        fn event(&mut self, event: SessionEvent) -> Result<(), String> {
            self.0.event(event)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::assets::tests::{Choices, Device, all, library};
    use super::*;
    use crate::claude::history::{self, kept};
    use fastctx_hosts::claude::transcript;
    use fastctx_types::session::attachments::NoReceipts;
    use fastctx_types::session::{Happened, SessionEvent};
    use serde_json::{Value, json};
    use std::collections::HashSet;

    const BASE: i64 = 1_800_000_000;

    fn at(second: i64) -> Timestamp {
        Timestamp::from_second(BASE + second).unwrap()
    }

    /// The lines of a Claude session's record numbered `session`: words and
    /// answers, tool calls and results — some unanswered, one answered
    /// twice — a row put again under its UUID, a retraction, a title, a
    /// compaction with its summary, a line that is no JSON; sometimes, last,
    /// a line holding more values than World reads (§28.49); first the rows
    /// of `copied`, a session before it, as a continuing session copies
    /// them.
    fn record(choices: &mut Choices, session: usize, copied: &[String]) -> Vec<String> {
        let mut lines: Vec<String> = copied.to_vec();
        let base = session as i64 * 1_000;
        let time = |n: i64| (jiff::Timestamp::from_second(BASE + base + n * 7).unwrap()).to_string();
        let mut parent = Value::Null;
        for n in 0..(8 + choices.pick(24) as i64) {
            let uuid = format!("s{session}-r{n}");
            let row = match choices.pick(10) {
                0 | 1 => json!({"type": "user", "uuid": uuid, "parentUuid": parent, "timestamp": time(n), "message": {"role": "user", "content": format!("words {session}-{n}")}}),
                2 => json!({"type": "assistant", "uuid": uuid, "parentUuid": parent, "timestamp": time(n),
                            "message": {"id": format!("m{session}-{n}"), "role": "assistant", "model": "m", "content": [{"type": "text", "text": format!("answer {n}")}]}}),
                3 | 4 => {
                    let call = format!("call-{session}-{}", choices.pick(6));
                    json!({"type": "assistant", "uuid": uuid, "parentUuid": parent, "timestamp": time(n),
                           "message": {"id": format!("m{session}-{n}"), "role": "assistant", "model": "m", "content": [{"type": "tool_use", "id": call, "name": "Bash", "input": {"command": "ls"}}]}})
                }
                5 => {
                    let call = format!("call-{session}-{}", choices.pick(6));
                    json!({"type": "user", "uuid": uuid, "parentUuid": parent, "timestamp": time(n),
                           "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": call, "content": "out"}]}})
                }
                6 => json!({"type": "custom-title", "customTitle": format!("title {session}-{n}")}),
                7 => {
                    let back = format!("s{session}-r{}", choices.pick(n.max(1) as u64));
                    json!({"type": "assistant", "uuid": uuid, "parentUuid": parent, "timestamp": time(n), "supersedesUuids": [back],
                           "message": {"id": format!("m{session}-{n}"), "role": "assistant", "model": "m", "content": [{"type": "text", "text": "again"}]}})
                }
                8 => {
                    // A compaction, the summary it begins with after it,
                    // naming it as its parent (`summarized`).
                    let boundary = json!({"type": "system", "subtype": "compact_boundary", "uuid": uuid, "logicalParentUuid": parent, "timestamp": time(n), "compactMetadata": {}});
                    lines.push(boundary.to_string());
                    json!({"type": "user", "uuid": format!("{uuid}-summary"), "parentUuid": uuid, "isCompactSummary": true, "timestamp": time(n),
                           "message": {"role": "user", "content": format!("summary {session}-{n}")}})
                }
                _ => {
                    // A row put again under a UUID it had.
                    let again = format!("s{session}-r{}", choices.pick(n.max(1) as u64));
                    json!({"type": "user", "uuid": again, "timestamp": time(n), "message": {"role": "user", "content": format!("put again {n}")}})
                }
            };
            if row.get("uuid").is_some() && choices.pick(4) != 0 {
                parent = row["uuid"].clone();
            }
            lines.push(row.to_string());
            if choices.pick(12) == 0 {
                lines.push("not json".into());
            }
        }
        if choices.pick(5) == 0 {
            lines.push(format!("[{}]", vec!["0"; fastctx_hosts::bounded::VALUES_MOST + 1].join(",")));
        }
        lines
    }

    /// Lines a record grows by that a reading past its seam takes as they
    /// are (`record` without compactions, retractions, rows put again or a
    /// line too large): words, answers, tool calls and their results, the
    /// session's numbering `session`, later than those of `record`'s.
    fn plain(choices: &mut Choices, session: usize) -> Vec<String> {
        let time = |n: i64| (jiff::Timestamp::from_second(BASE + session as i64 * 1_000 + n * 7).unwrap()).to_string();
        (0..4 + choices.pick(12) as i64)
            .map(|n| {
                let uuid = format!("s{session}-r{n}");
                let call = format!("call-{session}-{}", choices.pick(4));
                match choices.pick(4) {
                    0 => json!({"type": "user", "uuid": uuid, "timestamp": time(n), "message": {"role": "user", "content": format!("words {session}-{n}")}}),
                    1 => json!({"type": "assistant", "uuid": uuid, "timestamp": time(n),
                                "message": {"id": format!("m{session}-{n}"), "role": "assistant", "model": "m", "content": [{"type": "text", "text": format!("answer {n}")}]}}),
                    2 => json!({"type": "assistant", "uuid": uuid, "timestamp": time(n),
                                "message": {"id": format!("m{session}-{n}"), "role": "assistant", "model": "m", "content": [{"type": "tool_use", "id": call, "name": "Bash", "input": {"command": "ls"}}]}}),
                    _ => json!({"type": "user", "uuid": uuid, "timestamp": time(n), "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": call, "content": "out"}]}}),
                }
                .to_string()
            })
            .collect()
    }

    /// The World's record of the sessions' turns: each started and ended,
    /// words heard in some, some as a session's record has them.
    fn recorded(choices: &mut Choices, sessions: usize) -> Vec<SessionEvent> {
        let end: fastctx_types::session::TurnEnd = serde_json::from_value(json!({"how": "completed", "reason": null, "duration_ms": null})).unwrap();
        let mut recorded = Vec::new();
        for session in 0..sessions as i64 {
            for k in 0..choices.pick(4) as i64 {
                let turn = format!("t{session}-{k}");
                let start = session * 1_000 + k * 60;
                recorded.push(SessionEvent { at: at(start), happened: Happened::TurnStarted { turn: turn.clone() } });
                if choices.pick(2) == 0 {
                    let text = format!("words {session}-{}", choices.pick(30));
                    recorded.push(SessionEvent { at: at(start), happened: Happened::Heard { delivery: None, turn: Some(turn.clone()), from: None, text, message: None, notice: None } });
                }
                if choices.pick(4) != 0 {
                    recorded.push(SessionEvent { at: at(start + 50), happened: Happened::TurnEnded { turn, end: end.clone() } });
                }
            }
        }
        recorded
    }

    /// What `claude::history::hosted` and `talk::assemble` make of the
    /// sessions with all held in memory (no images, no context to restore,
    /// no receipts), whether a record stopped short, and the sets of what
    /// such records' events are known by.
    fn held(sessions: &[(String, Timestamp, std::path::PathBuf)], recorded: &[SessionEvent]) -> (Vec<SessionEvent>, bool, Vec<HashSet<String>>) {
        let (mut events, mut earlier, mut refused, mut frozen) = (Vec::new(), HashSet::new(), false, Vec::new());
        for (id, began, path) in sessions {
            let read = transcript::read_continuing(path, &[], &earlier).unwrap();
            earlier.extend(read.uuids.iter().cloned());
            let mut shown = Vec::new();
            if let Some(title) = read.title.clone() {
                shown.push(SessionEvent { at: *began, happened: Happened::Titled { title } });
            }
            shown.extend(read.events);
            if let Some(text) = history::problem_note(id, &read.problems, read.problems_omitted) {
                shown.push(history::note(*began, text));
            }
            if read.refused.is_some() {
                refused = true;
                let mut kept = HashSet::new();
                history::freeze(&mut kept, &shown);
                frozen.push(kept);
            }
            events.extend(shown);
        }
        let placed = history::place(events, recorded);
        (crate::talk::merged(recorded.to_vec(), Some(placed), "claude", &NoReceipts), refused, frozen)
    }

    /// The sessions' history read into a work store and written from there
    /// (design §28.51 ⑤-4) is the history held whole written: the same
    /// items, keys, bodies and objects, the same counts and floor — over
    /// sessions continuing earlier ones, records stopped short, attempts
    /// read again after the record grew, and a timeline that had a history.
    #[test]
    fn a_history_read_into_a_work_store_is_the_history_held() {
        let base = std::env::temp_dir().join(format!("fastctx-claude-kept-{}", std::process::id()));
        let signer: Arc<dyn fastctx_content::v2::Signer> = Arc::new(Device);
        let resident: ResidentId = "claude-a0a0".parse().unwrap();
        let none = HashSet::new();
        let mut choices = Choices(0x9e37_79b9_7f4a_7c15);
        let mut coverage = [0usize; 4];
        for round in 0..24 {
            let dir = base.join(format!("round-{round}"));
            let _ = fastctx_platform::disk::remove_dir_all(&dir);
            fastctx_platform::disk::create_dir_all(&dir).unwrap();
            let count = 1 + choices.pick(3) as usize;
            let mut sessions = Vec::new();
            let mut grown = Vec::new();
            let mut before: Vec<String> = Vec::new();
            for session in 0..count {
                // A continuing session copies some of the one before.
                let copied: Vec<String> = before.iter().filter(|_| choices.pick(2) == 0).cloned().collect();
                let lines = record(&mut choices, session, &copied);
                let path = dir.join(format!("session-{session}.jsonl"));
                fastctx_platform::disk::write(&path, lines.join("\n") + "\n").unwrap();
                before = lines.into_iter().filter(|line| line.starts_with('{')).collect();
                if choices.pick(3) == 0 {
                    grown.push(session);
                }
                sessions.push((format!("session-{session}"), at(session as i64 * 1_000), path));
            }
            let recorded = recorded(&mut choices, count);
            let (history, refused, frozen) = held(&sessions, &recorded);
            coverage[0] += usize::from(refused);
            coverage[1] += usize::from(count > 1);
            coverage[2] += usize::from(!grown.is_empty());
            coverage[3] += usize::from(history.iter().any(|event| matches!(&event.happened, Happened::Finished { item, .. } if matches!(item.what, fastctx_types::session::What::Compaction))));
            let span = |refused: bool, frozen: &[HashSet<String>]| -> Vec<HashSet<String>> { if refused { frozen.to_vec() } else { Vec::new() } };
            let (held_library, held_placed) = library(&dir.join("held"));
            let (kept_library, kept_placed) = library(&dir.join("kept"));
            // What the timeline had: a history of the first session alone.
            let (first, ..) = held(&sessions[..1], &recorded);
            for (library, placed) in [(&held_library, &held_placed), (&kept_library, &kept_placed)] {
                assets::write(library, signer.clone(), &resident, placed, &first, Span::Whole, &none, None).unwrap();
            }
            let held_sets = span(refused, &frozen);
            let held_span = if refused { Span::Added(&held_sets) } else { Span::Whole };
            let wrote = assets::write(&held_library, signer.clone(), &resident, &held_placed, &history, held_span, &none, None).unwrap();

            let spill_root = dir.join("spill");
            let mut reading = kept::Reading::new(super::super::spill::Spill::new(spill_root.clone(), "claude-a0a0")).unwrap();
            let mut kept_refused = false;
            for (index, (id, began, path)) in sessions.iter().enumerate() {
                if grown.contains(&index) {
                    // An attempt at a shorter record first, let go when it
                    // is read again after it grew.
                    let full = fastctx_platform::disk::read(path).unwrap();
                    let cut = full.iter().take(full.len() / 2).rposition(|byte| *byte == b'\n').map_or(0, |end| end + 1);
                    fastctx_platform::disk::write(path, &full[..cut]).unwrap();
                    reading.read(path, &[], &mut |_| {}).unwrap();
                    fastctx_platform::disk::write(path, &full).unwrap();
                }
                let read = reading.read(path, &[], &mut |_| {}).unwrap();
                kept_refused |= read.refused.is_some();
                reading.accept(id, *began, &read).unwrap();
            }
            assert_eq!(kept_refused, refused);
            let placed = reading.place(crate::claude::history::kept::Placing::Held { recorded: &recorded, merged: &recorded }, &mut |_| Ok(()), &NoReceipts, None, &|_| false).unwrap();
            let kept::Placed { spill, keys, names, .. } = placed;
            // What the spill kept is looked into with the timeline's test
            // support on (`--features timeline-test-support`).
            #[cfg(feature = "timeline-test-support")]
            let spill = {
                let mut spill = spill;
                assert_eq!(spill.known_sets(frozen.len()).unwrap(), frozen, "round {round}");
                spill
            };
            let mut finals = finals(None, Hosted { spill, keys, names, early: None }, "claude", &|_| false, crate::talk::Recorded::Held(recorded.clone()), refused).unwrap();
            let reach = if refused { Reach::Added } else { Reach::Whole };
            let kept = assets::write_kept(&kept_library, signer.clone(), &resident, &kept_placed, &mut finals.spill, None, reach, &mut |_, _, _| Ok(false)).unwrap();
            let floor = wrote.items.iter().map(|(_, _, key)| *key).max().unwrap_or(0);
            assert_eq!((kept.written, kept.gone, kept.floor), (wrote.written, wrote.gone, floor), "round {round}");
            assert_eq!(all(&kept_library), all(&held_library), "round {round}");
            drop(finals);
            let _ = super::super::spill::clear(&spill_root);
        }
        assert!(coverage.iter().all(|count| *count >= 2), "{coverage:?}");
        let _ = fastctx_platform::disk::remove_dir_all(&base);
    }

    /// The record ids before a seam kept beside the checkpoint as digests
    /// (`stage_prefix`, design §28.51 ⑤-7c) are the ids the checkpoint held
    /// whole before: read whole, those the reading moved past alone; read
    /// on, those before with them — over readings whole and on in any
    /// order, digests met again, batches past one; asked of a batch at a
    /// time (`digests_among`), those kept and no others.
    #[test]
    fn the_ids_before_a_seam_kept_beside_are_the_ids_held_whole() {
        let base = std::env::temp_dir().join(format!("fastctx-prefix-kept-{}", std::process::id()));
        let _ = fastctx_platform::disk::remove_dir_all(&base);
        let (library, _) = library(&base);
        let name = "timeline-ids/claude-a0a0";
        let mut choices = Choices(0x1319_8a2e_0370_7344);
        let mut held: Vec<u64> = Vec::new();
        let mut named = None;
        for round in 0..60 {
            let whole = round == 0 || choices.pick(3) == 0;
            let count = if choices.pick(8) == 0 { 5_000 + choices.pick(4_000) } else { choices.pick(40) };
            let moved: Vec<u64> = (0..count)
                .map(|_| {
                    let range = if choices.pick(2) == 0 { 200 } else { u64::MAX };
                    choices.pick(range)
                })
                .collect();
            // As the checkpoint held them whole before.
            if whole {
                held = moved.clone();
            } else if !moved.is_empty() {
                held.extend(&moved);
            }
            held.sort_unstable();
            held.dedup();
            if whole || !moved.is_empty() {
                named = Some(stage_prefix(&library, name, named, whole, &moved).unwrap());
            }
            assert_eq!(library.digests_of(name, named.unwrap()).unwrap(), held, "round {round}");
            let asked: Vec<u64> = (0..50)
                .map(|_| {
                    if choices.pick(2) == 0 && !held.is_empty() {
                        let at = choices.pick(held.len() as u64) as usize;
                        held[at]
                    } else {
                        choices.pick(u64::MAX)
                    }
                })
                .collect();
            let among = library.digests_among(name, named.unwrap(), &asked).unwrap();
            for digest in &asked {
                assert_eq!(among.contains(digest), held.binary_search(digest).is_ok(), "round {round}");
            }
        }
        // Gone with the conversation, all of them.
        library.drop_checkpoint(name).unwrap();
        assert!(library.digests_of(name, named.unwrap()).unwrap().is_empty());
        drop(library);
        let _ = fastctx_platform::disk::remove_dir_all(&base);
    }

    /// The floor a reading starts from, of the items the last one held in
    /// its window kept beside the checkpoint (`Held`, `floor_from`), is the
    /// floor those items give held whole — the highest of the last floor
    /// and the keys of those before the start not told again — for every
    /// start, over times alike and out of order, keys across all of u64;
    /// and a checkpoint is kept only in the epoch it was read in.
    #[test]
    fn the_floor_of_the_items_kept_is_the_floor_they_give() {
        let base = std::env::temp_dir().join(format!("fastctx-held-floor-{}", std::process::id()));
        let _ = fastctx_platform::disk::remove_dir_all(&base);
        let (library, _) = library(&base);
        let name = "timeline/claude-a0a0";
        let mut choices = Choices(0x6a09_e667_f3bc_c908);
        let mut generation = 0u64;
        for round in 0..300 {
            generation += 1;
            let wide = choices.pick(4) == 0;
            let items: Vec<(Timestamp, u64, bool)> = (0..choices.pick(60))
                .map(|_| {
                    let at = at(choices.pick(30) as i64) + jiff::SignedDuration::from_nanos(choices.pick(3) as i64);
                    let key = if wide { choices.pick(u64::MAX) } else { choices.pick(200) };
                    (at, key, choices.pick(5) == 0)
                })
                .collect();
            let floor = if wide { choices.pick(u64::MAX) } else { choices.pick(100) };
            // As `place` keeps them: those told again left out.
            let mut kept = Held::new(&library, name, generation);
            for (at, key, _) in items.iter().filter(|(.., told)| !told) {
                kept.push(*at, *key).unwrap();
            }
            assert_eq!(kept.finish().unwrap(), generation);
            let stood = Stood { name: name.to_string(), held: Some(generation), told: Vec::new(), floor, holding: None, generation: generation + 1 };
            for second in -1..32 {
                for nanos in 0..4 {
                    let since = at(second) + jiff::SignedDuration::from_nanos(nanos);
                    let whole = items.iter().filter(|(at, _, told)| *at < since && !told).map(|(_, key, _)| *key).fold(floor, u64::max);
                    assert_eq!(floor_from(&library, &stood, since).unwrap(), whole, "round {round}, since {since}");
                }
            }
            library.sweep_held(name, Some(generation)).unwrap();
        }
        // Read in an epoch, kept in it; let go meanwhile, kept no more.
        let (_, epoch) = library.checkpoint_in_epoch(name).unwrap();
        assert!(library.keep_checkpoint_in(name, b"one".to_vec(), epoch).unwrap());
        library.drop_checkpoint(name).unwrap();
        assert!(library.held_items(name, generation).unwrap().is_empty(), "what it held goes with it");
        assert!(!library.keep_checkpoint_in(name, b"two".to_vec(), epoch).unwrap());
        assert_eq!(library.checkpoint(name).unwrap(), None);
        let (_, after) = library.checkpoint_in_epoch(name).unwrap();
        assert!(library.keep_checkpoint_in(name, b"three".to_vec(), after).unwrap());
        drop(library);
        let _ = fastctx_platform::disk::remove_dir_all(&base);
    }

    /// A followed record read from its start into a work store (design
    /// §28.51 ⑤-5: the earlier sessions read and accepted there,
    /// `kept::Reading::tail` for the last, placed there, written by
    /// `place_kept`) leaves the timeline as the window held in memory does
    /// (`follow::tail` past the earlier sessions held, their events and the
    /// last's as `session_events` makes them, `history::place`,
    /// `talk::merged`, `place`): the same items and counts, floor, items
    /// held in the window and told again; the same seam, times, latest time
    /// before it and record ids moved past.
    #[test]
    fn a_followed_record_read_into_a_work_store_is_the_window_held() {
        use fastctx_hosts::claude::transcript::follow;
        let base = std::env::temp_dir().join(format!("fastctx-claude-follow-kept-{}", std::process::id()));
        let signer: Arc<dyn fastctx_content::v2::Signer> = Arc::new(Device);
        let resident: ResidentId = "claude-a0a0".parse().unwrap();
        let none = HashSet::new();
        let mut choices = Choices(0x51_7cc1_b727_220a);
        // How often: several sessions, a seam moved, items held in the
        // window, a note or title told again, a window read on from a seam.
        let mut coverage = [0usize; 5];
        let mut compared = 0;
        for round in 0..160 {
            let dir = base.join(format!("round-{round}"));
            let _ = fastctx_platform::disk::remove_dir_all(&dir);
            fastctx_platform::disk::create_dir_all(&dir).unwrap();
            let count = 1 + choices.pick(3) as usize;
            let mut sessions = Vec::new();
            let mut before: Vec<String> = Vec::new();
            for session in 0..count {
                let copied: Vec<String> = before.iter().filter(|_| choices.pick(2) == 0).cloned().collect();
                let lines = record(&mut choices, session, &copied);
                let path = dir.join(format!("session-{session}.jsonl"));
                fastctx_platform::disk::write(&path, lines.join("\n") + "\n").unwrap();
                before = lines.into_iter().filter(|line| line.starts_with('{')).collect();
                sessions.push((format!("session-{session}"), at(session as i64 * 1_000), path));
            }
            let recorded = recorded(&mut choices, count);
            let turns = history::turns(&recorded);
            let accept = |at: Timestamp| turns.iter().all(|(start, end)| *start >= at || end.is_some_and(|end| end <= at));
            let (last, earlier) = sessions.split_last().unwrap();
            let several = !earlier.is_empty();
            let shown = |id: &str, began: Timestamp, read: &transcript::Transcript, events: Vec<SessionEvent>| {
                let mut shown = Vec::new();
                if let Some(title) = read.title.clone() {
                    shown.push(SessionEvent { at: began, happened: Happened::Titled { title } });
                }
                shown.extend(events);
                let problem = history::problem_note(id, &read.problems, read.problems_omitted);
                if let Some(text) = &problem {
                    shown.push(history::note(began, text.clone()));
                }
                (shown, problem)
            };

            // Held: the earlier sessions as `earlier` reads them — all of
            // each, or the record is read whole as ever (none followed).
            let (mut held_events, mut copied) = (Vec::new(), HashSet::new());
            let mut stopped = Vec::new();
            for (id, began, path) in earlier {
                let read = transcript::read_continuing(path, &[], &copied).unwrap();
                stopped.push(read.refused.is_some());
                copied.extend(read.uuids.iter().cloned());
                let events = read.events.clone();
                held_events.extend(shown(id, *began, &read, events).0);
            }
            // Kept: the same sessions read into the store and accepted.
            let spill_root = dir.join("spill");
            let mut reading = kept::Reading::new(super::super::spill::Spill::new(spill_root.clone(), "claude-a0a0")).unwrap();
            for ((id, began, path), stopped) in earlier.iter().zip(&stopped) {
                let read = reading.read(path, &[], &mut |_| {}).unwrap();
                assert_eq!(read.refused.is_some(), *stopped, "round {round}");
                reading.accept(id, *began, &read).unwrap();
            }
            if stopped.contains(&true) {
                continue;
            }
            let mut seam = follow::Seam::default();
            let mut kept_seam = follow::Seam::default();
            if several {
                seam.latest = held_events.iter().map(|event| event.at).max();
                kept_seam.latest = reading.latest();
                assert_eq!(kept_seam.latest, seam.latest, "round {round}");
            }
            let (id, began, path) = last;
            let held_tail = follow::tail(path, &seam, &copied, &accept, false, &|_| false).unwrap().unwrap();
            let asked = vec![format!("s{}-r3", count - 1), "none".to_string()];
            let kept_tail = reading.tail(path, &kept_seam, &accept, false, &|_| Ok(Default::default()), &asked, &history::PrefixIds::digest, &mut |_| {}).unwrap().unwrap();
            assert_eq!(kept_tail.transcript.refused.is_some(), held_tail.transcript.refused.is_some(), "round {round}");
            if held_tail.transcript.refused.is_some() {
                continue;
            }
            compared += 1;
            coverage[0] += usize::from(several);
            coverage[1] += usize::from(held_tail.seam_at.is_some());
            let sets = |seam: &Option<follow::Seam>| {
                seam.clone().map(|mut seam| {
                    seam.abandoned.sort();
                    seam.told.sort();
                    seam.unlinked.sort();
                    serde_json::to_value(seam).unwrap()
                })
            };
            assert_eq!(sets(&kept_tail.seam), sets(&held_tail.seam), "round {round}");
            assert_eq!((kept_tail.since, kept_tail.seam_at, kept_tail.end), (held_tail.since, held_tail.seam_at, held_tail.end), "round {round}");
            assert_eq!(
                asked.iter().map(|uuid| kept_tail.transcript.uuids.contains(uuid)).collect::<Vec<_>>(),
                asked.iter().map(|uuid| held_tail.transcript.uuids.contains(uuid)).collect::<Vec<_>>(),
                "round {round}"
            );
            // The record ids before the seam, as the checkpoint keeps them.
            let mut held_ids: Vec<u64> = held_tail.prefix_ids.iter().map(|uuid| history::PrefixIds::digest(uuid)).collect();
            if several {
                held_ids.extend(copied.iter().map(|uuid| history::PrefixIds::digest(uuid)));
                reading.keep_earlier_moved(&history::PrefixIds::digest).unwrap();
            }
            held_ids.sort_unstable();
            held_ids.dedup();
            assert_eq!(reading.moved().unwrap(), held_ids, "round {round}");

            // The window held: placed, merged and written as `produce` does.
            let (last_shown, problem) = shown(id, *began, &held_tail.transcript, held_tail.transcript.events.clone());
            held_events.extend(last_shown);
            let title = held_tail.transcript.title.clone();
            let is_told = |event: &SessionEvent| match &event.happened {
                Happened::Titled { title: told } => event.at == *began && title.as_deref() == Some(told.as_str()),
                Happened::Health { health, .. } => event.at == *began && problem.as_deref() == Some(health.text.as_str()),
                _ => false,
            };
            let placed = history::place(held_events, &recorded);
            let held_told: Vec<SessionEvent> = placed.iter().filter(|event| is_told(event)).cloned().collect();
            coverage[3] += usize::from(!held_told.is_empty());
            let history = crate::talk::merged(recorded.clone(), Some(placed), "claude", &NoReceipts);
            let host_told: Vec<Value> = held_told.iter().map(|event| serde_json::to_value(event).unwrap()).collect();
            let settled = held_tail.seam_at.unwrap_or(Timestamp::MIN);
            let (held_library, held_placed) = library(&dir.join("held"));
            let (kept_library, kept_placed) = library(&dir.join("kept"));
            // What the timeline had: a history of the first session alone.
            let (first, ..) = held(&sessions[..1], &recorded);
            for (library, placed) in [(&held_library, &held_placed), (&kept_library, &kept_placed)] {
                assets::write(library, signer.clone(), &resident, placed, &first, Span::Whole, &none, None).unwrap();
            }
            let stood = Stood { name: assets::checkpoint(&resident), held: None, told: Vec::new(), floor: 0, holding: None, generation: 1 };
            let reading_held = Reading { history, host_told: host_told.clone(), whole: true, since: Timestamp::MIN, settled, codex: false };
            let held_placement = place(&held_library, signer.clone(), &resident, &held_placed, &stood, reading_held).unwrap();

            // The window kept: accepted, placed in the store, written.
            reading.accept(id, *began, &kept_tail.transcript).unwrap();
            let kept_placed_history = reading.place(crate::claude::history::kept::Placing::Held { recorded: &recorded, merged: &recorded }, &mut |_| Ok(()), &NoReceipts, None, &is_told).unwrap();
            assert_eq!(serde_json::to_value(&kept_placed_history.told).unwrap(), serde_json::to_value(&held_told).unwrap(), "round {round}");
            let told = |event: &SessionEvent| serde_json::to_value(event).is_ok_and(|value| host_told.contains(&value));
            let finals = finals(None, Kept::Claude { placed: kept_placed_history, early: None }.into(), "claude", &told, crate::talk::Recorded::Held(recorded.clone()), false).unwrap();
            let kept_placement = place_kept(&kept_library, signer.clone(), &resident, &kept_placed, &stood, finals, true, false, Timestamp::MIN, settled).unwrap();

            // What each kept of the items it holds in its window.
            let open = [
                held_library.held_items(&stood.name, held_placement.held).unwrap(),
                kept_library.held_items(&stood.name, kept_placement.held).unwrap(),
            ];
            coverage[2] += usize::from(!open[0].is_empty());
            let mut told_ids = [held_placement.told.clone(), kept_placement.told.clone()];
            told_ids.iter_mut().for_each(|told| told.sort());
            assert_eq!(
                (kept_placement.wrote.written, kept_placement.wrote.gone, kept_placement.floor),
                (held_placement.wrote.written, held_placement.wrote.gone, held_placement.floor),
                "round {round}"
            );
            assert_eq!(open[1], open[0], "round {round}");
            assert_eq!(told_ids[1], told_ids[0], "round {round}");
            let held_bytes: Vec<u8> = held_ids.iter().flat_map(|digest| digest.to_le_bytes()).collect();
            assert_eq!(kept_placement.before_seam, Some(held_bytes), "round {round}");
            assert_eq!(all(&kept_library), all(&held_library), "round {round}");
            let _ = super::super::spill::clear(&spill_root);

            // The record grows, and is read on from the seam each placed
            // (design §28.51 ⑤-7b): a window, held in memory and written by
            // `place`, or read into a work store and written by
            // `place_kept`, each from where its own timeline stood.
            let Some(seam) = held_tail.seam.clone().filter(|seam| seam.offset > 0) else { continue };
            let grown = plain(&mut choices, count + 10);
            let mut text = fastctx_platform::disk::read(path).unwrap();
            text.extend((grown.join("\n") + "\n").into_bytes());
            fastctx_platform::disk::write(path, &text).unwrap();
            let prefix = history::PrefixIds(held_ids.clone());
            let seen = |uuid: &str| prefix.holds(uuid);
            let held_on = follow::tail(path, &seam, &HashSet::new(), &accept, false, &seen).unwrap();
            let mut reading = kept::Reading::new(super::super::spill::Spill::new(spill_root.clone(), "claude-a0a0")).unwrap();
            let seen_kept = |uuids: &[String]| -> std::io::Result<HashSet<String>> { Ok(uuids.iter().filter(|uuid| prefix.holds(uuid)).cloned().collect()) };
            let kept_on = reading.tail(path, &seam, &accept, false, &seen_kept, &asked, &history::PrefixIds::digest, &mut |_| {}).unwrap();
            let (held_on, kept_on) = match (held_on, kept_on) {
                (Ok(held_on), Ok(kept_on)) => (held_on, kept_on),
                (held_on, kept_on) => {
                    assert_eq!(kept_on.err(), held_on.err(), "round {round}, read on");
                    continue;
                }
            };
            if held_on.transcript.refused.is_some() {
                assert!(kept_on.transcript.refused.is_some(), "round {round}");
                continue;
            }
            coverage[4] += 1;
            assert_eq!(sets(&kept_on.seam), sets(&held_on.seam), "round {round}, read on");
            assert_eq!((kept_on.since, kept_on.seam_at, kept_on.end), (held_on.since, held_on.seam_at, held_on.end), "round {round}, read on");
            let mut moved_on: Vec<u64> = held_on.prefix_ids.iter().map(|uuid| history::PrefixIds::digest(uuid)).collect();
            moved_on.sort_unstable();
            moved_on.dedup();
            assert_eq!(reading.moved().unwrap(), moved_on, "round {round}, read on");
            let since = held_on.since.unwrap_or(Timestamp::MAX);
            let settled = held_on.seam_at.unwrap_or(since);
            let merged: Vec<SessionEvent> = recorded.iter().filter(|event| event.at >= since).cloned().collect();
            let (shown_on, problem) = shown(id, *began, &held_on.transcript, held_on.transcript.events.clone());
            let title = held_on.transcript.title.clone();
            let is_told = |event: &SessionEvent| match &event.happened {
                Happened::Titled { title: told } => event.at == *began && title.as_deref() == Some(told.as_str()),
                Happened::Health { health, .. } => event.at == *began && problem.as_deref() == Some(health.text.as_str()),
                _ => false,
            };
            let placed = history::place(shown_on, &recorded);
            let held_told: Vec<SessionEvent> = placed.iter().filter(|event| is_told(event)).cloned().collect();
            let history = crate::talk::merged(merged.clone(), Some(placed), "claude", &NoReceipts);
            let host_told: Vec<Value> = held_told.iter().map(|event| serde_json::to_value(event).unwrap()).collect();
            let stood_of = |placement: &Placement| Stood {
                name: assets::checkpoint(&resident),
                held: Some(placement.held),
                told: placement.told.clone(),
                floor: placement.floor,
                holding: Some(placement.wrote.holding),
                generation: placement.held + 1,
            };
            let reading_on = Reading { history, host_told: host_told.clone(), whole: false, since, settled, codex: false };
            let held_window = place(&held_library, signer.clone(), &resident, &held_placed, &stood_of(&held_placement), reading_on).unwrap();
            reading.accept(id, *began, &kept_on.transcript).unwrap();
            let kept_history = reading.place(crate::claude::history::kept::Placing::Held { recorded: &recorded, merged: &merged }, &mut |_| Ok(()), &NoReceipts, None, &is_told).unwrap();
            assert_eq!(serde_json::to_value(&kept_history.told).unwrap(), serde_json::to_value(&held_told).unwrap(), "round {round}, read on");
            let told = |event: &SessionEvent| serde_json::to_value(event).is_ok_and(|value| host_told.contains(&value));
            let finals = super::finals(None, Kept::Claude { placed: kept_history, early: None }.into(), "claude", &told, crate::talk::Recorded::Held(merged.clone()), false).unwrap();
            let kept_window =
                place_kept(&kept_library, signer.clone(), &resident, &kept_placed, &stood_of(&kept_placement), finals, false, false, since, settled).unwrap();
            let name = assets::checkpoint(&resident);
            let mut told_ids = [held_window.told.clone(), kept_window.told.clone()];
            told_ids.iter_mut().for_each(|told| told.sort());
            assert_eq!(
                (kept_window.wrote.written, kept_window.wrote.gone, kept_window.floor),
                (held_window.wrote.written, held_window.wrote.gone, held_window.floor),
                "round {round}, read on"
            );
            assert_eq!(kept_library.held_items(&name, kept_window.held).unwrap(), held_library.held_items(&name, held_window.held).unwrap(), "round {round}, read on");
            assert_eq!(told_ids[1], told_ids[0], "round {round}, read on");
            assert_eq!(all(&kept_library), all(&held_library), "round {round}, read on");
            let _ = super::super::spill::clear(&spill_root);
        }
        assert!(compared >= 24 && coverage.iter().all(|count| *count >= 2), "{compared} compared, {coverage:?}");
        let _ = fastctx_platform::disk::remove_dir_all(&base);
    }
}
