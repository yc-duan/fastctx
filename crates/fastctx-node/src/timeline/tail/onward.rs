//! A timeline written on from a place inside a turn (sync-v2-impl-design
//! §10). A reading from the seam (`produce_step`) judges the whole window
//! past it: every row read again, every event placed, merged and compared
//! with the timeline. Between two such readings, a Claude record that only
//! grew is read from the last place it was read through
//! (`claude::history::onward`) — a row that begins an answer of the model's
//! with no tool call waiting — and only what the rows from there on show is
//! made again and written. The items the window holds before that place
//! stay as the reading from the seam wrote them.
//!
//! Nothing is settled here. The seam, the floor and the generation the
//! window's items are held under stay as the reading from the seam left
//! them; what cannot be shown from the place on alone — the World's record
//! grown, words heard, a row that refers back before the place, an event
//! the World's record tells of too, a record changed before it — is left to
//! the next reading from the seam, which makes the window again. What a
//! window stands on is held in memory only (`Window`): a node that stops
//! reads from the seam once.

use super::*;
use crate::claude::history;
use crate::talk::merging;
use fastctx_content::v2::{Kind as _, Library, Timeline};
use fastctx_platform::readings;
use fastctx_types::session::{Happened, SessionChange};
use std::collections::HashSet;
use std::path::PathBuf;

/// How long a window is written on before a reading from the seam judges it
/// again whatever the record shows: what the two leave is compared then
/// (`Audit`). One whose reading from the seam reads the record whole — a
/// compaction lies past the seam — is judged less often.
const JUDGED_EVERY: Duration = Duration::from_secs(15 * 60);
const JUDGED_EVERY_WHOLE: Duration = Duration::from_secs(2 * 60 * 60);
/// The most a window holds of what the rows before the place leave, and of
/// items past it: beyond either it is read from the seam at every look, as
/// it was before windows were written on. A row leaves two or three, eight
/// bytes each and as much again to hold them.
const LEFT_MOST: usize = 1 << 19;
const ITEMS_MOST: usize = 4096;
/// The most windows held at once, and the most all of them hold of what
/// rows leave: the one judged longest ago goes first.
const WINDOWS_MOST: usize = 128;
const LEFT_MOST_OF_ALL: usize = 1 << 22;
/// The timeline's last items read at once to compare (`seed`).
const COMPARED_AT_ONCE: u32 = 64;
const COMPARED_BYTES: u32 = 8 << 20;

/// What a window written on stands on (the module's doc).
pub(super) struct Window {
    /// The checkpoint as it was kept when the window was last written, by
    /// the digest of its bytes: kept otherwise since, the window is let go.
    stood: [u8; 32],
    session: String,
    /// The place the record is read through, and what the rows between the
    /// seam and it leave for those after them (`history::left_key`).
    read: Seam,
    left: Arc<HashSet<u64>>,
    /// The World's turn the events past the place are placed in: of those
    /// under way, the last begun (`World::running`).
    turn: Option<String>,
    /// The names the World's record merged with the window names, closed:
    /// an event of the host's the World's record names too is merged with
    /// it by a reading from the seam.
    names: merging::Names,
    /// The session's title as the record told it.
    title: Option<String>,
    /// The order key of the last item before those of the rows from the
    /// place on.
    floor: u64,
    /// The items written of the rows from the place on, in their order:
    /// identity, time and order key.
    items: Vec<(Vec<u8>, Timestamp, u64)>,
    /// When a reading from the seam last judged the window, and whether it
    /// read the record whole.
    judged: Instant,
    whole: bool,
}

static WINDOWS: LazyLock<Mutex<HashMap<ResidentId, Window>>> = LazyLock::new(Mutex::default);

fn windows() -> std::sync::MutexGuard<'static, HashMap<ResidentId, Window>> {
    WINDOWS.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn keep(resident: &ResidentId, window: Window) {
    let mut windows = windows();
    windows.remove(resident);
    let mut left: usize = windows.values().map(|held| held.left.len()).sum::<usize>() + window.left.len();
    while windows.len() >= WINDOWS_MOST || (left > LEFT_MOST_OF_ALL && !windows.is_empty()) {
        let Some(oldest) = windows.iter().min_by_key(|(_, held)| held.judged).map(|(resident, _)| resident.clone()) else { break };
        left -= windows.remove(&oldest).map_or(0, |held| held.left.len());
    }
    windows.insert(resident.clone(), window);
}

/// Lets go every window held: what each stands on is made again by the next
/// reading from its seam.
pub(crate) fn forget() {
    windows().clear();
}

/// Lets go `resident`'s window.
pub(super) fn forget_of(resident: &ResidentId) {
    windows().remove(resident);
}

fn digest(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

/// Why a look was left to a reading from the seam, counted by its kind.
fn left_to_the_seam(why: &'static str) {
    let kind: String = why.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' }).collect();
    readings::counter_of("timeline.onward.left", &kind).add(1);
}

/// What a look written on asks the reading from the seam that follows it at
/// once to confirm (`JUDGED_EVERY`): the record, the World's record and the
/// facts of runs standing as the look read them, that reading writes
/// nothing — what it writes is where the two differ.
pub(super) struct Audit {
    path: PathBuf,
    end: u64,
    latest: u64,
    runs: i64,
}

impl Audit {
    /// Whether the reading from the seam read what the look read: the
    /// record no longer, the World's record and the runs' facts as they
    /// stood. Call on a thread that may block.
    pub(super) fn stands(&self, latest: u64, runs: i64) -> bool {
        latest == self.latest && runs == self.runs && fastctx_platform::disk::metadata(&self.path).is_ok_and(|metadata| metadata.len() == self.end)
    }

    /// The reading from the seam wrote `written` items and took `gone` away
    /// of what the look left: counted and told.
    pub(super) fn differed(node: &Node, resident: &ResidentId, written: usize, gone: usize) {
        readings::counter("timeline.onward.disagreed").add(1);
        node.log.line(format_args!(
            "{resident}'s timeline written on from inside a turn was not as a reading from its seam makes it: that reading wrote {written} item(s) and took {gone} away"
        ));
    }
}

/// What a look did (`advance`).
pub(super) enum Step {
    /// Written on: the timeline is brought to the record.
    Done(Produced),
    /// Written on, and due to be judged by a reading from the seam now.
    Judge(Produced, Audit),
    /// Not written on: a reading from the seam brings it.
    Seam,
}

/// Whether `event`, of a row past the place, is one written on from there:
/// an item finished, a change that tells no other history, a note — none
/// the World's record places or tells itself (words, a turn's start or
/// end, a request).
fn alone(event: &SessionEvent) -> bool {
    match &event.happened {
        Happened::Finished { item, .. } => item.draft.is_none(),
        Happened::Changed { change, .. } => {
            !matches!(change, SessionChange::HistoryChanged | SessionChange::HistoryCut { .. } | SessionChange::Request { .. } | SessionChange::Resolved { .. })
        }
        Happened::Health { .. } => true,
        _ => false,
    }
}

/// The history the events of the rows past a place make, as a reading from
/// the seam makes it of them among the window's: each later than `after`,
/// the latest time before the place; placed in `turn` in the order of their
/// times (`placement::decide`), shown (`talk::shown`), merged with a World's
/// record that tells of none of them (`names`) and given the facts of its
/// runs (where there are the conversation's files to tell them). Why not,
/// where one of them is not made so alone. Call on a thread that may block.
fn derive(
    directory: Option<&fastctx_types::layout::ConversationDir>,
    mut events: Vec<SessionEvent>,
    turn: Option<&str>,
    names: &mut merging::Names,
    after: Option<Timestamp>,
) -> Result<Result<Vec<SessionEvent>, &'static str>, Failure> {
    use history::placement::{self, Turns};
    // In the order of their times, those alike in time as they came.
    events.sort_by_key(|event| event.at);
    let turns = Turns::kept(Vec::new(), &|_, _| false);
    let mut made = Vec::with_capacity(events.len());
    for event in events {
        if after.is_some_and(|after| event.at <= after) {
            return Ok(Err("an event no later than the place"));
        }
        if !alone(&event) {
            return Ok(Err("an event placed by the World's record"));
        }
        let Some(event) = placement::finish(event, turn, &turns) else { continue };
        for mut shown in crate::talk::shown(event, &fastctx_types::session::attachments::NoReceipts, "claude", &|_| false) {
            let key = merging::key(&shown, names);
            let named = |name: Option<u32>| name.is_some_and(|name| name != merging::ELSEWHERE);
            if matches!(key.kind, merging::Kind::Finished | merging::Kind::Updated) && named(key.item) {
                return Ok(Err("an item the World's record tells of too"));
            }
            if !key.in_history || crate::talk::changes_history(&shown) {
                continue;
            }
            if let Some(directory) = directory {
                crate::run_facts::attach(directory, &mut shown).map_err(crate::record_migration::failure)?;
            }
            made.push(shown);
        }
    }
    Ok(Ok(made))
}

/// Where the record ids before `resident`'s seam are kept, for a reading on.
async fn seen_of(library: &Library, resident: &ResidentId) -> Result<Option<(Library, String, u64)>, Failure> {
    Ok(prefix_generation(library, resident).await?.map(|generation| (library.clone(), prefix_name(resident), generation)))
}

/// An item of a window: its identity, time and order key.
type Item = (Vec<u8>, Timestamp, u64);

/// The timeline's last items where they are those `made` makes, in its
/// order — the same identities, order keys rising, the same records — with
/// the order key of the item before them (naught where none is); none where
/// they are not. Read from its end a few at a time.
fn last_items(library: &Library, scope: &str, made: &[SessionEvent]) -> Result<Option<(Vec<Item>, u64)>, Failure> {
    use fastctx_content::{Anchor, Place};
    let identities = super::super::names::identities(made);
    let key_of = |order: &[u8]| <[u8; 8]>::try_from(order).ok().map(u64::from_be_bytes);
    let mut items: Vec<Item> = Vec::with_capacity(made.len());
    let mut remaining = made.len();
    let mut anchor = Anchor::Latest;
    let floor = loop {
        let want = if remaining == 0 { 1 } else { remaining.min(COMPARED_AT_ONCE as usize) as u32 };
        let page = library.view_page(scope, Timeline::STREAM, &anchor, want, COMPARED_BYTES).map_err(failure)?;
        let Some(first) = page.entries.first() else {
            if remaining == 0 {
                break 0;
            }
            return Ok(None);
        };
        if remaining == 0 {
            let Some(floor) = page.entries.last().and_then(|last| key_of(&last.order)) else { return Ok(None) };
            break floor;
        }
        anchor = Anchor::Before(Place { order: first.order.clone(), item: first.item.clone() });
        for entry in page.entries.iter().rev() {
            let Some(index) = remaining.checked_sub(1) else { break };
            remaining = index;
            let event = &made[index];
            let encoded = serde_json::to_vec(event).map_err(failure)?;
            let (body, object) = assets::body(event, &encoded)?;
            let objects: Vec<fastctx_content::ObjectRef> = object.into_iter().collect();
            let Some(key) = key_of(&entry.order) else { return Ok(None) };
            if entry.item != identities[index] || entry.body != body || entry.objects != objects {
                return Ok(None);
            }
            items.push((entry.item.clone(), event.at, key));
        }
    };
    items.reverse();
    let rising = items.first().is_none_or(|(_, _, key)| *key > floor) && items.windows(2).all(|pair| pair[0].2 < pair[1].2);
    Ok(rising.then_some((items, floor)))
}

/// What a reading from the seam leaves for a window written on (`seeded`):
/// the place it read the record through and what the rows before it leave,
/// the World's turn under way and the names its record names, and whether
/// the record was read whole.
pub(super) struct Judged {
    pub(super) read: Seam,
    pub(super) left: Vec<u64>,
    pub(super) turn: Option<String>,
    pub(super) names: merging::Names,
    pub(super) whole: bool,
}

/// What the rows from a place on gave at one look (`history::read_on`):
/// their events, where the place moved to and what the rows it moved past
/// leave, and the session's title as the record tells it.
pub(super) struct Read {
    pub(super) events: Vec<SessionEvent>,
    pub(super) moved: Option<Seam>,
    pub(super) passed: Vec<u64>,
    pub(super) title: Option<String>,
}

/// What a look written on wrote (`Window::write_on`): how many items it
/// wrote and took away, the holding period it wrote in, and whether the
/// window is still to be held.
pub(super) struct Written {
    pub(super) written: usize,
    pub(super) gone: usize,
    pub(super) holding: i64,
    pub(super) holds: bool,
}

impl Window {
    /// The place moved to `next`, past rows that leave `passed`: the items
    /// of the rows before it — those no later than the latest time before
    /// it — are the window's own from now on, the last of them the floor
    /// of those past it. Whether it is still held: not past `LEFT_MOST`.
    fn moved(&mut self, next: Seam, passed: Vec<u64>) -> bool {
        Arc::make_mut(&mut self.left).extend(passed);
        self.read = next;
        let after = self.read.latest;
        let before = self.items.partition_point(|(_, at, _)| after.is_some_and(|after| *at <= after));
        if let Some((_, _, key)) = before.checked_sub(1).and_then(|last| self.items.get(last)) {
            self.floor = *key;
        }
        self.items.drain(..before);
        self.left.len() <= LEFT_MOST
    }

    /// After a reading from the seam brought the timeline in `scope` and
    /// kept the checkpoint whose bytes' digest is `stood`: the window to
    /// write on from the place that reading read the record through
    /// (`judged`), given what the rows from that place on show now (`read`)
    /// — held only where the timeline's last items are those these rows make
    /// alone (`derive`): the same identities in the same order with the
    /// same records, so that what a look written on makes is what that
    /// reading made. Why not, otherwise: an event of the World's record
    /// among them, words, an item both tell. Call on a thread that may
    /// block.
    pub(super) fn seeded(
        library: &Library,
        scope: &str,
        directory: Option<&fastctx_types::layout::ConversationDir>,
        stood: [u8; 32],
        session: String,
        judged: Judged,
        read: Read,
    ) -> Result<Result<Window, &'static str>, Failure> {
        let Judged { read: place, left, turn, mut names, whole } = judged;
        let Read { events, moved, passed, title } = read;
        let made = match derive(directory, events, turn.as_deref(), &mut names, place.latest)? {
            Ok(made) => made,
            Err(why) => return Ok(Err(why)),
        };
        if made.is_empty() {
            return Ok(Err("nothing past the place"));
        }
        if made.len() > ITEMS_MOST || left.len() > LEFT_MOST {
            return Ok(Err("a window too long to hold"));
        }
        let Some((items, floor)) = last_items(library, scope, &made)? else { return Ok(Err("a timeline's end the rows past the place do not make")) };
        let mut window =
            Window { stood, session, read: place, left: Arc::new(left.into_iter().collect()), turn, names, title, floor, items, judged: Instant::now(), whole };
        if let Some(next) = moved
            && !window.moved(next, passed)
        {
            return Ok(Err("a window too long to hold"));
        }
        Ok(Ok(window))
    }

    /// What the rows from the window's place on show now (`read`) made
    /// (`derive`) and written in place of the items written of them before
    /// — past the window's floor, as `place` writes a window past its own —
    /// and the items it holds kept so beside the checkpoint `name` under
    /// `generation`, as a reading from the seam keeps them (`Held`); the
    /// place moved on where the reading moved it. Written only in the
    /// holding period `expected`. Why not, where the rows cannot be made
    /// alone: nothing was written then. Call on a thread that may block.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn write_on(
        &mut self,
        library: &Library,
        signer: Arc<dyn fastctx_content::v2::Signer>,
        resident: &ResidentId,
        placed: &assets::Placed,
        directory: Option<&fastctx_types::layout::ConversationDir>,
        name: &str,
        generation: u64,
        expected: Option<i64>,
        read: Read,
    ) -> Result<Result<Written, &'static str>, Failure> {
        let Read { events, moved, passed, title } = read;
        if title != self.title {
            return Ok(Err("the session's title"));
        }
        let made = match derive(directory, events, self.turn.as_deref(), &mut self.names, self.read.latest)? {
            Ok(made) => made,
            Err(why) => return Ok(Err(why)),
        };
        if made.len() > ITEMS_MOST {
            return Ok(Err("a window too long to hold"));
        }
        // Named apart from those before the place: an event new here that
        // the timeline has an item of already is one of the same name
        // before it, told apart only over the whole window.
        let known: HashSet<&Vec<u8>> = self.items.iter().map(|(identity, ..)| identity).collect();
        let fresh: Vec<Vec<u8>> = super::super::names::identities(&made).into_iter().filter(|identity| !known.contains(identity)).collect();
        if !fresh.is_empty() && !library.had_items(&placed.scope, Timeline::STREAM, &fresh).map_err(failure)?.is_empty() {
            return Ok(Err("an event named as an item before the place"));
        }
        let before: Vec<(Vec<u8>, Timestamp)> = self.items.iter().map(|(identity, at, _)| (identity.clone(), *at)).collect();
        let span = Span::From { since: Timestamp::MIN, before: &before, floor: self.floor };
        let wrote = assets::write(library, signer, resident, placed, &made, span, &HashSet::new(), expected)?;
        // From here on the timeline is written.
        let rising = wrote.items.first().is_none_or(|(_, _, key)| *key > self.floor) && wrote.items.windows(2).all(|pair| pair[0].2 < pair[1].2);
        let pair = |(_, at, key): &Item| (at.as_second(), at.subsec_nanosecond(), *key);
        let (old, new): (HashSet<_>, HashSet<_>) = (self.items.iter().map(pair).collect(), wrote.items.iter().map(pair).collect());
        let (gone, kept): (Vec<_>, Vec<_>) = (old.difference(&new).copied().collect(), new.difference(&old).copied().collect());
        if !gone.is_empty() || !kept.is_empty() {
            library.restage_held(name, generation, gone, kept).map_err(failure)?;
        }
        self.items = wrote.items;
        let mut holds = rising;
        if let Some(next) = moved {
            holds &= self.moved(next, passed);
        }
        Ok(Ok(Written { written: wrote.written, gone: wrote.gone, holding: wrote.holding, holds }))
    }
}

#[cfg(feature = "bench")]
impl Window {
    /// What the rows of the record at `path` from the place `judged` read
    /// it through show now (`history::read_on`), for `seeded`; `seen`
    /// where the record ids before the seam are kept.
    pub(super) fn read_from(path: &std::path::Path, judged: &Judged, seen: Option<&(Library, String, u64)>) -> Result<Read, String> {
        let held: HashSet<u64> = judged.left.iter().copied().collect();
        Self::read_past(path, &judged.read, &held, seen)
    }

    fn read_past(path: &std::path::Path, place: &Seam, held: &HashSet<u64>, seen: Option<&(Library, String, u64)>) -> Result<Read, String> {
        let (tail, events, passed) = history::read_on(path, place, held, seen, &[], false, &mut |_| {})
            .map_err(|error| error.to_string())?
            .map_err(str::to_string)?
            .map_err(|why| why.to_string())?;
        Ok(Read { events, moved: tail.seam, passed, title: tail.transcript.title })
    }

    /// One look written on of the record at `path`, as `advance` reads and
    /// writes it with the World's record and the runs' facts as they stood.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn look(
        &mut self,
        path: &std::path::Path,
        library: &Library,
        signer: Arc<dyn fastctx_content::v2::Signer>,
        resident: &ResidentId,
        placed: &assets::Placed,
        name: &str,
        generation: u64,
        expected: Option<i64>,
        seen: Option<&(Library, String, u64)>,
    ) -> Result<Written, String> {
        let read = Self::read_past(path, &self.read, &self.left, seen)?;
        self.write_on(library, signer, resident, placed, None, name, generation, expected, read).map_err(|failure| failure.message)?.map_err(str::to_string)
    }
}

/// After a reading from the seam brought `resident`'s timeline and kept
/// `next` (`kept`, its bytes): its window to write on, where one is to be
/// held (`Window::seeded`), of what the rows from the place that reading
/// read the record through show now.
#[allow(clippy::too_many_arguments)]
pub(super) async fn seed(
    node: &Arc<Node>,
    resident: &ResidentId,
    library: &Library,
    scope: &str,
    next: &Following,
    kept: &[u8],
    judged: Judged,
    producing: super::super::serial::Held,
) {
    forget_of(resident);
    let Some(session) = next.session.clone() else { return };
    if judged.left.len() > LEFT_MOST {
        return left_to_the_seam("a window too long to hold");
    }
    let held: Arc<HashSet<u64>> = Arc::new(judged.left.iter().copied().collect());
    let Ok(seen) = seen_of(library, resident).await else { return };
    let onward = match history::onward(node, resident, &session, &next.earlier, &judged.read, held, seen).await {
        Ok(Ok(onward)) => onward,
        Ok(Err(why)) => return left_to_the_seam(why),
        Err(_) => return left_to_the_seam("a record not read"),
    };
    let history::Onward { events, next: moved, passed, title, .. } = onward;
    let (library, scope, directory, stood) = (library.clone(), scope.to_string(), node.world.conversation(resident), digest(kept));
    let seeded = fastctx_platform::task::blocking(move || {
        let _held = producing;
        Window::seeded(&library, &scope, Some(&directory), stood, session, judged, Read { events, moved, passed, title })
    })
    .await;
    match seeded {
        Ok(Ok(window)) => {
            readings::counter("timeline.onward.seeded").add(1);
            keep(resident, window);
        }
        Ok(Err(why)) => left_to_the_seam(why),
        Err(_) => left_to_the_seam("a record not read"),
    }
}

/// Brings `resident`'s timeline on from the place its window was read
/// through, where it holds one that stands on the checkpoint `state` (its
/// bytes' digest `stood`) and nothing but the record grew: the rows from
/// the place on read (`history::onward`), what they show written
/// (`Window::write_on`), the facts of runs that changed given to the items
/// before the place, and the checkpoint kept again at `version`, its seam
/// and floor as they were. `Step::Seam` where any of that cannot be:
/// nothing was written then.
#[allow(clippy::too_many_arguments)]
pub(super) async fn advance(
    node: &Arc<Node>,
    resident: &ResidentId,
    about: &fastctx_types::residents::About,
    library: &Library,
    name: &str,
    epoch: u64,
    state: &Following,
    stood: [u8; 32],
    version: &str,
    runs_now: i64,
    producing: super::super::serial::Held,
) -> Result<Step, Failure> {
    let Some(window) = windows().remove(resident) else {
        left_to_the_seam("no window held");
        return Ok(Step::Seam);
    };
    let leave = |why: &'static str| {
        left_to_the_seam(why);
        Ok(Step::Seam)
    };
    let (Some(generation), true) = (state.held, window.stood == stood && state.session.as_deref() == Some(window.session.as_str())) else {
        return leave("a checkpoint kept otherwise");
    };
    let directory = node.world.conversation(resident);
    // What the World recorded since is placed and merged by a reading from
    // the seam: a turn begun or ended, words heard.
    let latest = {
        let directory = directory.clone();
        fastctx_platform::task::blocking(move || crate::session_record::latest(&directory)).await.map_err(failure)?
    };
    if state.world_latest != Some(latest) {
        return leave("the World's record grown");
    }
    let seen = seen_of(library, resident).await?;
    let onward = match history::onward(node, resident, &window.session, &state.earlier, &window.read, window.left.clone(), seen).await {
        Ok(Ok(onward)) => onward,
        Ok(Err(why)) => return leave(why),
        Err(_) => return leave("a record not read"),
    };
    let history::Onward { events, next: moved, passed, title, kept, kept_here, end, path, .. } = onward;
    // The last record its mod saw kept: among the rows read now, or those
    // between the seam and the place; found in neither, a reading from the
    // seam waits for it.
    let kept_here = kept_here || kept.as_ref().is_some_and(|kept| window.left.contains(&history::left_key(&format!("{}{kept}", fastctx_hosts::claude::transcript::follow::KEY_ROW))));
    if kept.is_some() && !kept_here && kept != state.kept {
        return leave("a record its mod saw kept not yet in the file");
    }
    let Some(placed) = assets::placed(node, about)? else { return leave("no scope") };
    let (hub, scope) = (placed.hub.clone(), placed.scope.clone());
    let signer = node.assets.as_ref().map(|kept| kept.signer()).ok_or_else(|| Failure::new("no asset library is kept here"))?;
    let (written, mut window) = {
        let (library, whose, directory, held, signer, name, expected) =
            (library.clone(), resident.clone(), directory.clone(), producing.clone(), signer.clone(), name.to_string(), state.holding);
        let mut window = window;
        fastctx_platform::task::blocking(move || -> Result<_, Failure> {
            let _held = held;
            let read = Read { events, moved, passed, title };
            let written = window.write_on(&library, signer, &whose, &placed, Some(&directory), &name, generation, expected, read)?;
            Ok((written, window))
        })
        .await?
    };
    let written = match written {
        Ok(written) => written,
        Err(why) => return leave(why),
    };
    // The facts of runs changed since the timeline last had them: those of
    // the rows past the place were given as they were made; the items
    // before it are given theirs in place, as a reading from the seam gives
    // those before its window.
    let (runs_version, refreshed) =
        runs_brought(node, about, library, signer, resident, &directory, producing.clone(), state.runs_version, runs_now, window.floor, written.holding).await?;
    let next = Following {
        version: version.to_string(),
        holding: Some(written.holding),
        kept: if kept_here { kept } else { state.kept.clone() },
        runs_version,
        ..state.clone()
    };
    let long_changed = next.runs_version.is_none();
    let bytes = serde_json::to_vec(&next).map_err(failure)?;
    window.stood = digest(&bytes);
    publish(library, name, bytes, epoch, producing).await?;
    let (written, gone, holds) = (written.written + refreshed, written.gone, written.holds);
    if written + gone > 0 {
        crate::assets::committed(node, &hub, &scope);
    }
    readings::counter("timeline.onward.looks").add(1);
    let produced = Produced { written, gone, unread: false, long_changed, verified: state.verified, handing: false, read: false };
    if !holds {
        left_to_the_seam("a window too long to hold");
        return Ok(Step::Done(produced));
    }
    let due = window.judged.elapsed() >= if window.whole { JUDGED_EVERY_WHOLE } else { JUDGED_EVERY };
    keep(resident, window);
    if due {
        return Ok(Step::Judge(produced, Audit { path, end, latest, runs: runs_now }));
    }
    Ok(Step::Done(produced))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claude::history::kept;
    use crate::timeline::assets::tests::{Choices, Device, all, library};
    use fastctx_types::session::attachments::NoReceipts;
    use serde_json::{Value, json};

    const BASE: i64 = 1_800_000_000;

    fn at(second: i64) -> Timestamp {
        Timestamp::from_second(BASE + second).unwrap()
    }

    /// A timeline brought by readings from the seam, as `produce_step`
    /// brings one of a record read into a work store: where it stands.
    struct Brought {
        library: Library,
        placed: assets::Placed,
        seam: Seam,
        held: Option<u64>,
        told: Vec<String>,
        floor: u64,
        holding: Option<i64>,
    }

    /// Where the record ids before the seam are kept beside the checkpoint.
    fn seen_in(library: &Library, resident: &ResidentId) -> Option<(Library, String, u64)> {
        let name = prefix_name(resident);
        let bytes = library.checkpoint(&name).unwrap()?;
        Some((library.clone(), name, u64::from_le_bytes(bytes.as_slice().try_into().unwrap())))
    }

    impl Brought {
        fn new(dir: &std::path::Path) -> Self {
            let (library, placed) = library(dir);
            Brought { library, placed, seam: Seam::default(), held: None, told: Vec::new(), floor: 0, holding: None }
        }

        /// One reading from the seam, placed and written as `produce_step`
        /// does; read whole where the seam's tail cannot be read alone.
        /// What it leaves for a window written on, and whether the seam
        /// moved or the record was read whole.
        fn judge(&mut self, spill: &std::path::Path, path: &std::path::Path, recorded: &[SessionEvent], resident: &ResidentId) -> (Option<Judged>, bool) {
            let signer: Arc<dyn fastctx_content::v2::Signer> = Arc::new(Device);
            let turns = history::turns(recorded);
            let accept = |at: Timestamp| turns.iter().all(|(start, end)| *start >= at || end.is_some_and(|end| end <= at));
            let began = at(0);
            let before = self.seam.offset;
            loop {
                let _ = crate::timeline::spill::clear(spill);
                let mut reading = kept::Reading::new(crate::timeline::spill::Spill::new(spill.to_path_buf(), "claude-a0a0")).unwrap();
                let seen_kept = seen_in(&self.library, resident);
                let seen = |uuids: &[String]| -> std::io::Result<HashSet<String>> {
                    let Some((library, name, generation)) = &seen_kept else { return Ok(HashSet::new()) };
                    let digests: Vec<u64> = uuids.iter().map(|uuid| history::PrefixIds::digest(uuid)).collect();
                    let held = library.digests_among(name, *generation, &digests).map_err(std::io::Error::other)?;
                    Ok(uuids.iter().zip(&digests).filter(|(_, digest)| held.contains(digest)).map(|(uuid, _)| uuid.clone()).collect())
                };
                let mut left = Vec::new();
                let read = reading
                    .tail(path, &self.seam, &accept, false, &seen, &[], &history::PrefixIds::digest, &mut |_| {}, &mut |key| {
                        left.push(history::left_key(key));
                        Ok(())
                    })
                    .unwrap();
                let tail = match read {
                    Ok(tail) => tail,
                    Err(_) if self.seam.identity.is_some() => {
                        self.seam = Seam::default();
                        continue;
                    }
                    Err(why) => panic!("not read whole: {why}"),
                };
                assert!(tail.transcript.refused.is_none());
                let whole = self.seam.identity.is_none();
                let since = if whole { Timestamp::MIN } else { tail.since.unwrap_or(Timestamp::MAX) };
                let settled = tail.seam_at.unwrap_or(since);
                let merged: Vec<SessionEvent> = recorded.iter().filter(|event| whole || event.at >= since).cloned().collect();
                reading.accept("session", began, &tail.transcript).unwrap();
                let title = tail.transcript.title.clone();
                let problem = history::problem_note("session", &tail.transcript.problems, tail.transcript.problems_omitted);
                let is_told = |event: &SessionEvent| match &event.happened {
                    Happened::Titled { title: told } => event.at == began && title.as_deref() == Some(told.as_str()),
                    Happened::Health { health, .. } => event.at == began && problem.as_deref() == Some(health.text.as_str()),
                    _ => false,
                };
                let placed = reading.place(kept::Placing::Held { recorded, merged: &merged }, &mut |_| Ok(()), &NoReceipts, None, &is_told).unwrap();
                let names = placed.names.clone();
                let host_told: Vec<Value> = placed.told.iter().map(|event| serde_json::to_value(event).unwrap()).collect();
                let told = |event: &SessionEvent| serde_json::to_value(event).is_ok_and(|value| host_told.contains(&value));
                let finals = finals(None, Kept::Claude { placed, early: None }.into(), "claude", &told, crate::talk::Recorded::Held(merged), false).unwrap();
                let name = assets::checkpoint(resident);
                let generation = self.held.map_or(1, |held| held.wrapping_add(1));
                let stood = Stood { name: name.clone(), held: self.held, told: self.told.clone(), floor: self.floor, holding: self.holding, generation };
                let placement = place_kept(&self.library, signer, resident, &self.placed, &stood, finals, whole, false, since, settled).unwrap();
                let digests: Vec<u64> = placement.before_seam.as_deref().unwrap_or_default().chunks_exact(8).map(|chunk| u64::from_le_bytes(chunk.try_into().unwrap())).collect();
                if whole || !digests.is_empty() {
                    let named = seen_in(&self.library, resident).map(|(_, _, generation)| generation);
                    stage_prefix(&self.library, &prefix_name(resident), named, whole, &digests).unwrap();
                }
                self.held = Some(placement.held);
                self.told = placement.told;
                self.floor = placement.floor;
                self.holding = Some(placement.wrote.holding);
                self.library.sweep_held(&name, self.held).unwrap();
                self.seam = tail.seam.clone().unwrap_or_else(|| self.seam.clone());
                // As `World::running`: of the turns under way, the last begun.
                let ended: HashSet<&str> = recorded.iter().filter_map(|event| if let Happened::TurnEnded { turn, .. } = &event.happened { Some(turn.as_str()) } else { None }).collect();
                let turn = recorded
                    .iter()
                    .filter_map(|event| match &event.happened {
                        Happened::TurnStarted { turn } if !ended.contains(turn.as_str()) => Some((event.at, turn.clone())),
                        _ => None,
                    })
                    .max_by_key(|(at, _)| *at)
                    .map(|(_, turn)| turn);
                let judged = tail.read.map(|read| Judged { read, left, turn, names, whole });
                let _ = crate::timeline::spill::clear(spill);
                return (judged, whole || self.seam.offset != before);
            }
        }
    }

    /// A record at work, written a few rows at a time: a turn the World
    /// began, rounds of the model's answers — thoughts, words, tool calls
    /// and their results under one message, each with its usage — and now
    /// and then what a look written on cannot show alone.
    struct Work {
        clock: i64,
        round: usize,
        turn: usize,
        open: bool,
        /// A tool call left waiting, answered at a later step.
        waiting: Option<String>,
    }

    impl Work {
        fn time(&mut self, choices: &mut Choices) -> String {
            self.clock += 1 + choices.pick(3) as i64;
            at(self.clock).to_string()
        }

        /// The model's next round: rows of one message.
        fn answers(&mut self, choices: &mut Choices, lines: &mut Vec<String>) {
            self.round += 1;
            let round = self.round;
            let message = format!("wm{round}");
            let usage = json!({"input_tokens": round, "output_tokens": 1});
            let mut part = 0;
            let row = |block: Value, at: String, lines: &mut Vec<String>, part: &mut usize| {
                lines.push(
                    json!({"type": "assistant", "uuid": format!("wa{round}-{part}"), "timestamp": at,
                           "message": {"id": message, "role": "assistant", "model": "x", "usage": usage, "content": [block]}})
                    .to_string(),
                );
                *part += 1;
            };
            if choices.pick(3) == 0 {
                let at = self.time(choices);
                row(json!({"type": "thinking", "thinking": "hm", "signature": "s"}), at, lines, &mut part);
            }
            if choices.pick(2) == 0 {
                let at = self.time(choices);
                row(json!({"type": "text", "text": format!("next {round}")}), at, lines, &mut part);
            }
            let calls: Vec<String> = (0..choices.pick(3)).map(|call| format!("wk{round}-{call}")).collect();
            for call in &calls {
                let at = self.time(choices);
                row(json!({"type": "tool_use", "id": call, "name": "Bash", "input": {"command": "ls"}}), at, lines, &mut part);
            }
            for (index, call) in calls.iter().enumerate() {
                // The last call of a round is now and then left waiting.
                if index + 1 == calls.len() && choices.pick(8) == 0 {
                    self.waiting = Some(call.clone());
                    break;
                }
                let at = self.time(choices);
                lines.push(self.result(call, at));
            }
        }

        fn result(&self, call: &str, at: String) -> String {
            json!({"type": "user", "uuid": format!("wr-{call}-{}", self.clock), "timestamp": at,
                   "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": call, "content": "out"}]}})
            .to_string()
        }

        /// What the record and the World's record grow by at one step.
        fn step(&mut self, choices: &mut Choices, lines: &mut Vec<String>, recorded: &mut Vec<SessionEvent>) -> &'static str {
            let end: fastctx_types::session::TurnEnd = serde_json::from_value(json!({"how": "completed", "reason": null, "duration_ms": null})).unwrap();
            if let Some(call) = self.waiting.take() {
                let at = self.time(choices);
                lines.push(self.result(&call, at));
                return "an answer waited for";
            }
            if !self.open {
                // The next turn: begun by the World, its words in the record.
                self.turn += 1;
                self.open = true;
                self.clock += 2;
                let turn = format!("t{}", self.turn);
                let text = format!("words {}", self.turn);
                recorded.push(SessionEvent { at: at(self.clock), happened: Happened::TurnStarted { turn: turn.clone() } });
                recorded.push(SessionEvent { at: at(self.clock), happened: Happened::Heard { delivery: None, turn: Some(turn), from: None, text: text.clone(), message: None, notice: None } });
                let at = self.time(choices);
                lines.push(json!({"type": "user", "uuid": format!("wp{}", self.turn), "timestamp": at, "message": {"role": "user", "content": text}}).to_string());
                return "a turn begun";
            }
            match choices.pick(40) {
                0 => {
                    self.clock += 1;
                    recorded.push(SessionEvent { at: at(self.clock), happened: Happened::TurnEnded { turn: format!("t{}", self.turn), end } });
                    self.open = false;
                    "a turn ended"
                }
                1 | 2 => {
                    // Words handed into the turn under way.
                    self.clock += 1;
                    let text = format!("more {}", self.clock);
                    recorded.push(SessionEvent {
                        at: at(self.clock),
                        happened: Happened::Heard { delivery: None, turn: Some(format!("t{}", self.turn)), from: None, text: text.clone(), message: None, notice: None },
                    });
                    let at = self.time(choices);
                    lines.push(json!({"type": "user", "uuid": format!("wq{}", self.clock), "timestamp": at, "message": {"role": "user", "content": text}}).to_string());
                    "words heard"
                }
                3 => {
                    lines.push(json!({"type": "custom-title", "customTitle": format!("title {}", self.clock)}).to_string());
                    "a title"
                }
                4 if self.round > 2 => {
                    // An answer taking back one before it.
                    let back = format!("wa{}-0", 1 + choices.pick(self.round as u64 - 1));
                    self.round += 1;
                    let at = self.time(choices);
                    lines.push(
                        json!({"type": "assistant", "uuid": format!("wa{}-0", self.round), "timestamp": at, "supersedesUuids": [back],
                               "message": {"id": format!("wm{}", self.round), "role": "assistant", "model": "x", "content": [{"type": "text", "text": "again"}]}})
                        .to_string(),
                    );
                    "an answer taken back"
                }
                5 => {
                    lines.push("not json".into());
                    "a line not made out"
                }
                6 => {
                    let uuid = format!("wc{}", self.clock);
                    let at = self.time(choices);
                    lines.push(json!({"type": "system", "subtype": "compact_boundary", "uuid": uuid, "logicalParentUuid": null, "timestamp": at, "compactMetadata": {}}).to_string());
                    lines.push(
                        json!({"type": "user", "uuid": format!("{uuid}-summary"), "parentUuid": uuid, "isCompactSummary": true, "timestamp": at,
                               "message": {"role": "user", "content": "summary"}})
                        .to_string(),
                    );
                    "a compaction"
                }
                7 if self.round > 2 => {
                    // A call of a round before answered again.
                    let call = format!("wk{}-0", 1 + choices.pick(self.round as u64 - 1));
                    let at = self.time(choices);
                    lines.push(self.result(&call, at));
                    "a call answered again"
                }
                _ => {
                    for _ in 0..1 + choices.pick(3) {
                        self.answers(choices, lines);
                        if self.waiting.is_some() {
                            break;
                        }
                    }
                    "answers"
                }
            }
        }
    }

    /// A timeline written on from inside a turn is the timeline a reading
    /// from the seam makes at every step: over a record at work that grows a
    /// few rows at a time — rounds of answers, tool calls answered and left
    /// waiting, words handed into the turn, turns ended and begun, titles,
    /// answers taken back, compactions, lines not made out — one timeline
    /// is brought by a reading from the seam at each step, the other
    /// written on from the place the last such reading read the record
    /// through wherever its window stands; both hold the same items, order
    /// keys, records and objects after every step, and, while their seams
    /// are one, the same items kept beside the checkpoint.
    #[test]
    fn a_window_written_on_is_the_window_read_from_the_seam() {
        let base = std::env::temp_dir().join(format!("fastctx-timeline-onward-{}", std::process::id()));
        let signer: Arc<dyn fastctx_content::v2::Signer> = Arc::new(Device);
        let resident: ResidentId = "claude-a0a0".parse().unwrap();
        let name = assets::checkpoint(&resident);
        let mut choices = Choices(0x9e37_79b9_7f4a_7c15);
        // How many steps were written on, and how many were left to the seam.
        let (mut on, mut seam, mut seeded) = (0usize, 0usize, 0usize);
        let mut met: HashMap<&'static str, (usize, usize)> = HashMap::new();
        let mut why_not: HashMap<String, usize> = HashMap::new();
        for round in 0..24 {
            let dir = base.join(format!("round-{round}"));
            let _ = fastctx_platform::disk::remove_dir_all(&dir);
            fastctx_platform::disk::create_dir_all(&dir).unwrap();
            let path = dir.join("session.jsonl");
            let mut work = Work { clock: 0, round: 0, turn: 0, open: false, waiting: None };
            let (mut lines, mut recorded) = (Vec::new(), Vec::new());
            let mut slow = Brought::new(&dir.join("slow"));
            let mut fast = Brought::new(&dir.join("fast"));
            let mut window: Option<Window> = None;
            let mut world = 0;
            for step in 0..60 {
                let grew = work.step(&mut choices, &mut lines, &mut recorded);
                fastctx_platform::disk::write(&path, lines.join("\n") + "\n").unwrap();
                slow.judge(&dir.join("spill-slow"), &path, &recorded, &resident);
                // Written on where a window stands and only the record grew.
                let mut written = false;
                if let Some(held) = window.as_mut()
                    && world == recorded.len()
                {
                    let seen = seen_in(&fast.library, &resident);
                    match history::read_on(&path, &held.read, &held.left, seen.as_ref(), &[], false, &mut |_| {}).unwrap() {
                        Ok(Ok((tail, events, passed))) => {
                            let read = Read { events, moved: tail.seam, passed, title: tail.transcript.title };
                            let wrote = held.write_on(&fast.library, signer.clone(), &resident, &fast.placed, None, &name, fast.held.unwrap(), fast.holding, read).unwrap();
                            written = wrote.is_ok();
                            if let Err(why) = &wrote {
                                *why_not.entry(format!("written: {why}")).or_default() += 1;
                            }
                            if wrote.is_ok_and(|wrote| !wrote.holds) {
                                window = None;
                            }
                        }
                        Ok(Err(why)) => *why_not.entry(format!("read: {why}")).or_default() += 1,
                        Err(why) => *why_not.entry(format!("read: {why}")).or_default() += 1,
                    }
                } else if window.is_some() {
                    *why_not.entry("the World's record grown".into()).or_default() += 1;
                } else {
                    *why_not.entry("no window".into()).or_default() += 1;
                }
                let count = met.entry(grew).or_default();
                if written {
                    on += 1;
                    count.0 += 1;
                } else {
                    seam += 1;
                    count.1 += 1;
                    window = None;
                    let (judged, _) = fast.judge(&dir.join("spill-fast"), &path, &recorded, &resident);
                    world = recorded.len();
                    match judged {
                        Some(judged) => {
                            let held: HashSet<u64> = judged.left.iter().copied().collect();
                            let seen = seen_in(&fast.library, &resident);
                            match history::read_on(&path, &judged.read, &held, seen.as_ref(), &[], false, &mut |_| {}).unwrap() {
                                Ok(Ok((tail, events, passed))) => {
                                    let read = Read { events, moved: tail.seam, passed, title: tail.transcript.title };
                                    match Window::seeded(&fast.library, &fast.placed.scope, None, [0; 32], "session".into(), judged, read).unwrap() {
                                        Ok(held) => {
                                            window = Some(held);
                                            seeded += 1;
                                        }
                                        Err(why) => *why_not.entry(format!("seeded: {why}")).or_default() += 1,
                                    }
                                }
                                Ok(Err(why)) => *why_not.entry(format!("seed read: {why}")).or_default() += 1,
                                Err(why) => *why_not.entry(format!("seed read: {why}")).or_default() += 1,
                            }
                        }
                        None => *why_not.entry("seeded: no place read through".into()).or_default() += 1,
                    }
                }
                assert_eq!(all(&fast.library), all(&slow.library), "round {round}, step {step}: {grew}, written on: {written}");
                if fast.seam.offset == slow.seam.offset {
                    assert_eq!(
                        fast.library.held_items(&name, fast.held.unwrap()).unwrap(),
                        slow.library.held_items(&name, slow.held.unwrap()).unwrap(),
                        "round {round}, step {step}: {grew}, written on: {written}"
                    );
                }
            }
            let _ = fastctx_platform::disk::remove_dir_all(&dir);
        }
        let _ = fastctx_platform::disk::remove_dir_all(&base);
        assert!(on >= 400 && seam >= 100 && seeded >= 60, "{on} written on, {seam} left to the seam, {seeded} windows held: {met:?}, {why_not:?}");
        assert!(met.get("answers").is_some_and(|(on, _)| *on >= 300), "{met:?}");
    }
}
