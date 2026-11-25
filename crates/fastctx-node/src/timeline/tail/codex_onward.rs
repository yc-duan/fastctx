//! A Codex thread's timeline written on from inside its running turn
//! (sync-v2-impl-design §10, §21), as `onward` writes a Claude record's. A
//! reading from the seam (`produce_step`) lists every entry of the turn
//! that runs and places, merges and compares all of them at every look.
//! Between two such readings, a thread whose running turn only gained
//! entries is listed from the last entry it was read through
//! (`codex::follow::onward`: an entry done, the turn's last when it was
//! read), and only what follows it is made and written: the new items, then
//! what the host tells again at every reading — the thread's own account of
//! itself and its title — with what the World records later than the place
//! put among them by the merge itself (`merge`).
//!
//! Nothing is settled here: the seam, the floor and the generation the
//! window's items are held under stay as the reading from the seam left
//! them. What cannot be shown from the place on alone — the turn ended,
//! words steered into it, an entry not done, a row of another turn, the
//! World's record telling what places others — is left to the next reading
//! from the seam. Prototyped and weighed against that reading by
//! codex-b482f509 (Codex 0.160.0: 20 of 20 listings alike over a real turn).

use super::*;
use crate::talk::{self, merging};
use fastctx_content::v2::{Kind as _, Library, Timeline};
use fastctx_hosts::codex::follow::onward::{Delta, Through};
use fastctx_types::session::{Happened, SessionChange, attachments::NoReceipts};
use std::collections::HashSet;

/// The most events of the World's record a window holds past its place, and
/// the most items: beyond either the thread is read from its seam.
const WORLD_MOST: usize = 128;
const ITEMS_MOST: usize = 2048;
/// The most windows held at once: the one judged longest ago goes first.
const WINDOWS_MOST: usize = 128;

/// A window held for a resident's thread: the checkpoint it stands on, by
/// the digest of its bytes, the thread, and when a reading from the seam
/// last judged it.
struct Held {
    stood: [u8; 32],
    thread: String,
    window: Window,
    judged: Instant,
}

static WINDOWS: LazyLock<Mutex<HashMap<ResidentId, Held>>> = LazyLock::new(Mutex::default);

fn windows() -> std::sync::MutexGuard<'static, HashMap<ResidentId, Held>> {
    WINDOWS.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Lets go every window held.
pub(super) fn forget() {
    windows().clear();
}

/// Lets go `resident`'s window.
pub(super) fn forget_of(resident: &ResidentId) {
    windows().remove(resident);
}

fn keep(resident: &ResidentId, held: Held) {
    let mut windows = windows();
    windows.remove(resident);
    while windows.len() >= WINDOWS_MOST {
        let Some(oldest) = windows.iter().min_by_key(|(_, held)| held.judged).map(|(resident, _)| resident.clone()) else { break };
        windows.remove(&oldest);
    }
    windows.insert(resident.clone(), held);
}

type Item = (Vec<u8>, Timestamp, u64);
type Refused = &'static str;

pub(super) struct Window {
    through: Through,
    names: merging::Names,
    sent: bool,
    floor: u64,
    world: Vec<SessionEvent>,
    suffix: Vec<Item>,
    told: HashSet<Vec<u8>>,
    host_told: Vec<SessionEvent>,
}

/// The history the host's events past a place (`hosted`, in the thread's
/// order), what it tells again (`told`) and what the World records later
/// than the place (`world`) make together, as `talk::assemble` makes it of
/// a whole window: shown, merged by the merge itself, each given the facts
/// of its runs (where there are the conversation's files to tell them).
fn merge(
    directory: Option<&fastctx_types::layout::ConversationDir>,
    mut hosted: Vec<SessionEvent>,
    told: &[SessionEvent],
    world: &[SessionEvent],
    sent: bool,
    turn: &str,
) -> Result<Vec<SessionEvent>, Failure> {
    hosted.extend_from_slice(told);
    let hosted: Vec<_> = hosted
        .into_iter()
        .flat_map(|event| talk::shown(event, &NoReceipts, "codex", &|id| id == turn && sent))
        .collect();
    let mut names = merging::Names::default();
    let hosted_keys: Vec<_> = hosted.iter().map(|event| merging::key(event, &mut names)).collect();
    let recorded_keys: Vec<_> = world.iter().map(|event| merging::key(event, &mut names)).collect();
    let slots = merging::decide(&hosted_keys, &recorded_keys, true);
    let mut made = merging::apply(slots, hosted, world.to_vec());
    made.retain(|event| !talk::changes_history(event));
    if let Some(directory) = directory {
        for event in &mut made {
            crate::run_facts::attach(directory, event).map_err(crate::record_migration::failure)?;
        }
    }
    Ok(made)
}

fn ignored(event: &SessionEvent) -> bool {
    matches!(
        event.happened,
        Happened::Changed { change: SessionChange::Request { .. } | SessionChange::Resolved { .. }, .. }
    )
}

fn safe_world(event: &SessionEvent, turn: &str) -> bool {
    match &event.happened {
        Happened::Context { .. } => true,
        Happened::Changed { turn: Some(who), change: SessionChange::Usage { .. } } => who == turn,
        _ => false,
    }
}

fn told_ids(made: &[SessionEvent], told: &[SessionEvent]) -> HashSet<Vec<u8>> {
    let same = |event: &SessionEvent| told_again(event) && told.contains(event);
    super::super::names::identities(made)
        .into_iter()
        .zip(made)
        .filter_map(|(id, event)| same(event).then_some(id))
        .collect()
}

impl Window {
    /// The order key of the last item before those the window writes.
    fn floor(&self) -> u64 {
        self.floor
    }

    pub(super) fn seed(
        library: &Library,
        scope: &str,
        directory: Option<&fastctx_types::layout::ConversationDir>,
        through: Through,
        recorded: &[SessionEvent],
        told: &[SessionEvent],
    ) -> Result<Result<Self, Refused>, Failure> {
        let world: Vec<_> =
            recorded.iter().filter(|event| event.at > through.latest() && !ignored(event)).cloned().collect();
        if world.len() > WORLD_MOST || world.iter().any(|event| !safe_world(event, through.turn())) {
            return Ok(Err("the judge's World suffix is not independent"));
        }
        let sent = recorded
            .iter()
            .any(|event| matches!(&event.happened, Happened::Heard { turn: Some(turn), .. } if turn == through.turn()));
        let suffix_events = merge(directory, Vec::new(), told, &world, sent, through.turn())?;
        let Some((suffix, floor)) = onward::last_items(library, scope, &suffix_events)? else {
            return Ok(Err("the store does not end with the candidate suffix"));
        };
        let host_told = told.to_vec();
        let told = told_ids(&suffix_events, told);
        Ok(Ok(Self { through, names: merging::Names::over(recorded), sent, floor, world, suffix, told, host_told }))
    }

    pub(super) fn through(&self) -> &Through {
        &self.through
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn write_on(
        &mut self,
        library: &Library,
        signer: Arc<dyn fastctx_content::v2::Signer>,
        resident: &ResidentId,
        placed: &assets::Placed,
        directory: Option<&fastctx_types::layout::ConversationDir>,
        checkpoint: &str,
        generation: u64,
        expected: Option<i64>,
        delta: Delta,
        recorded: &[SessionEvent],
    ) -> Result<Result<(assets::Wrote, Vec<String>), Refused>, Failure> {
        let Delta { through, events, told } = delta;
        let turn = self.through.turn();
        let at = self.through.latest();
        if through.turn() != turn || through.latest() < at || recorded.len() + self.world.len() > WORLD_MOST {
            return Ok(Err("the turn or the size of the World suffix changed"));
        }
        let mut world = self.world.clone();
        for event in recorded {
            if ignored(event) {
                continue;
            }
            if event.at <= at || !safe_world(event, turn) {
                return Ok(Err("the World added an event that requires the seam"));
            }
            world.push(event.clone());
        }
        for event in &events {
            if event.at <= at || talk::turn_of(&event.happened) != Some(turn) {
                return Ok(Err("an event reaches back across the boundary"));
            }
            if !matches!(&event.happened, Happened::Finished { item, .. } if item.draft.is_none()) {
                return Ok(Err("a host event needs more of the merge"));
            }
            let key = merging::key(event, &mut self.names);
            if key.item.is_some_and(|item| item != merging::ELSEWHERE) {
                return Ok(Err("a host item is also named by the World"));
            }
        }
        if events.is_empty() && self.host_told != told {
            return Ok(Err("metadata changed without a new host event"));
        }
        let made = merge(directory, events, &told, &world, self.sent, turn)?;
        if made.len() > ITEMS_MOST {
            return Ok(Err("the suffix exceeds its item budget"));
        }
        let world_after: Vec<_> = world.into_iter().filter(|event| event.at > through.latest()).collect();
        let next_suffix = merge(directory, Vec::new(), &told, &world_after, self.sent, turn)?;
        if !made.ends_with(&next_suffix) {
            return Ok(Err("the merged suffix cannot be split at the host boundary"));
        }

        let known: HashSet<_> = self.suffix.iter().map(|(id, ..)| id).collect();
        let identities = super::super::names::identities(&made);
        let fresh: Vec<_> = identities.iter().filter(|id| !known.contains(id)).cloned().collect();
        if !fresh.is_empty() && !library.had_items(&placed.scope, Timeline::STREAM, &fresh).map_err(failure)?.is_empty()
        {
            return Ok(Err("an item has the name of a previously placed event"));
        }
        let again = told_ids(&made, &told);
        let before: Vec<_> = self.suffix.iter().map(|(id, at, _)| (id.clone(), *at)).collect();
        let span = Span::From { since: Timestamp::MIN, before: &before, floor: self.floor };
        let wrote = assets::write(library, signer, resident, placed, &made, span, &again, expected)?;
        let held = |rows: &[Item], told: &HashSet<Vec<u8>>| -> HashSet<_> {
            rows.iter()
                .filter(|(id, ..)| !told.contains(id))
                .map(|(_, at, key)| (at.as_second(), at.subsec_nanosecond(), *key))
                .collect()
        };
        let old = held(&self.suffix, &self.told);
        let new = held(&wrote.items, &again);
        let gone: Vec<_> = old.difference(&new).copied().collect();
        let kept: Vec<_> = new.difference(&old).copied().collect();
        if !gone.is_empty() || !kept.is_empty() {
            library.restage_held(checkpoint, generation, gone, kept).map_err(failure)?;
        }
        let keep = wrote.items.len() - next_suffix.len();
        if let Some((_, _, key)) = keep.checked_sub(1).and_then(|index| wrote.items.get(index)) {
            self.floor = *key;
        }
        self.suffix = wrote.items[keep..].to_vec();
        self.through = through;
        self.world = world_after;
        self.told = again;
        self.host_told = told;
        let checkpoint_told = self.told.iter().map(|id| hex(id)).collect();
        Ok(Ok((wrote, checkpoint_told)))
    }
}

/// After a reading from the seam brought `resident`'s Codex timeline and
/// kept its checkpoint (`kept`, its bytes): its window to write on from the
/// place that reading read the thread through (`through`), held only where
/// the timeline's last items are what the host tells again and the World
/// records later than the place make (`Window::seed`). `recorded` is the
/// World's record as that reading merged it, `told` what the host told
/// again.
#[allow(clippy::too_many_arguments)]
pub(super) async fn seed(
    node: &Arc<Node>,
    resident: &ResidentId,
    library: &Library,
    scope: &str,
    kept: &[u8],
    through: Through,
    recorded: Vec<SessionEvent>,
    told: Vec<SessionEvent>,
    producing: super::super::serial::Held,
) {
    forget_of(resident);
    let Some(thread) = node.residents.about(resident).and_then(|(about, _)| about.session) else { return };
    let (library, scope, directory, stood) = (library.clone(), scope.to_string(), node.world.conversation(resident), onward::digest(kept));
    let seeded = fastctx_platform::task::blocking(move || {
        let _held = producing;
        Window::seed(&library, &scope, Some(&directory), through, &recorded, &told)
    })
    .await;
    match seeded {
        Ok(Ok(window)) => {
            fastctx_platform::readings::counter("timeline.onward.seeded").add(1);
            keep(resident, Held { stood, thread, window, judged: Instant::now() });
        }
        Ok(Err(why)) => onward::left_to_the_seam(why),
        Err(_) => onward::left_to_the_seam("a thread not read"),
    }
}

/// Brings `resident`'s Codex timeline on from the place its window was
/// read through, where it holds one that stands on the checkpoint `state`
/// (its bytes' digest `stood`): the entries past the place listed
/// (`CodexHost::read_onward`), what they and the World's record show made
/// and written (`Window::write_on`), the facts of runs that changed given
/// to the items before the place, and the checkpoint kept again at
/// `version` with what the host tells again, its seam and floor as they
/// were. `Step::Seam` where any of that cannot be: nothing was written
/// then.
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
) -> Result<onward::Step, Failure> {
    use onward::Step;
    let Some(held) = windows().remove(resident) else {
        onward::left_to_the_seam("no window held");
        return Ok(Step::Seam);
    };
    let leave = |why: &'static str| {
        onward::left_to_the_seam(why);
        Ok(Step::Seam)
    };
    let Held { stood: stands_on, thread, mut window, judged } = held;
    let (Some(generation), Some(heard), true) = (state.held, state.world_latest, stands_on == stood && about.session.as_deref() == Some(thread.as_str())) else {
        return leave("a checkpoint kept otherwise");
    };
    let codex = node.codex.of(node, about)?;
    // What Codex said so far is in the World's record before it is read, as
    // a reading from the seam has it.
    codex.flush_events(&thread).await.map_err(Failure::new)?;
    let directory = node.world.conversation(resident);
    let (latest, fresh) = match onward::recorded_since(&directory, heard).await? {
        (latest, Some(fresh)) => (latest, fresh),
        (_, None) => return leave("a window too long to hold"),
    };
    let owner = fastctx_types::Address { machine: node.machine(), resident: resident.clone() };
    let delta = match codex.read_onward(&thread, &owner, window.through()).await {
        Ok(Ok(delta)) => delta,
        Ok(Err(why)) => return leave(why),
        Err(_) => return leave("a thread not read"),
    };
    let Some(placed) = assets::placed(node, about)? else { return leave("no scope") };
    let (hub, scope) = (placed.hub.clone(), placed.scope.clone());
    let signer = node.assets.as_ref().map(|kept| kept.signer()).ok_or_else(|| Failure::new("no asset library is kept here"))?;
    let (written, window) = {
        let (library, whose, directory, held, signer, name, expected) =
            (library.clone(), resident.clone(), directory.clone(), producing.clone(), signer.clone(), name.to_string(), state.holding);
        fastctx_platform::task::blocking(move || -> Result<_, Failure> {
            let _held = held;
            let written = window.write_on(&library, signer, &whose, &placed, Some(&directory), &name, generation, expected, delta, &fresh)?;
            Ok((written, window))
        })
        .await?
    };
    let (wrote, told) = match written {
        Ok(written) => written,
        Err(why) => return leave(why),
    };
    let (runs_version, refreshed) =
        runs_brought(node, about, library, signer, resident, &directory, producing.clone(), state.runs_version, runs_now, window.floor(), wrote.holding).await?;
    let next = Following {
        version: version.to_string(),
        holding: Some(wrote.holding),
        told,
        runs_version,
        // Read through here: what it told is in the window.
        world_latest: Some(latest),
        ..state.clone()
    };
    let long_changed = next.runs_version.is_none();
    let bytes = serde_json::to_vec(&next).map_err(failure)?;
    let stood = onward::digest(&bytes);
    publish(library, name, bytes, epoch, producing).await?;
    let (written, gone) = (wrote.written + refreshed, wrote.gone);
    if written + gone > 0 {
        crate::assets::committed(node, &hub, &scope);
    }
    fastctx_platform::readings::counter("timeline.onward.looks").add(1);
    let produced = Produced { written, gone, unread: false, long_changed, verified: state.verified, handing: false, read: false, on: true, waits_listing: false };
    let due = judged.elapsed() >= onward::JUDGED_EVERY;
    keep(resident, Held { stood, thread, window, judged });
    if due {
        return Ok(Step::Judge(produced, onward::Audit::of_world(latest, runs_now)));
    }
    Ok(Step::Done(produced))
}

#[cfg(all(test, unix, feature = "bench"))]
mod tests;
