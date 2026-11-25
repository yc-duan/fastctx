//! What the other machines last said of their agents and their logins, kept
//! so a page that lists them shows them at once (`interaction-design.md`).
//!
//! The new conversation's agent picker and the quota page list what every
//! machine has. Asking every machine as the page opened, each hub's whole
//! directory read again first, made the page wait on the slowest of them.
//! Now each machine tells its hubs, with its heartbeat, a number that moves
//! whenever what it answers of its agents and logins does
//! (`fastctx_net::directory::Member::logins_number`), and a page is answered
//! with what this node last heard from each machine while that number
//! stands. A machine whose number moved, or one never heard from, is asked;
//! the page waits a moment for it (`SOON`), and is told to read again once
//! what it answers changed what was shown (`topics::LOGINS`). A machine its
//! hub says is away is told as away, unasked. Nothing is asked while no one
//! reads.
//!
//! What is kept is kept for an account as it was opened, and goes whole
//! when its hub's directory changes, not only who is online: what an
//! account may see of a machine is the directory's to say, and none of it
//! is told from before it changed (codex-580b4d87).

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use fastctx_net::around::{Asked, Heard, Reachable};
use fastctx_net::link::Link;
use fastctx_types::access::{HubId, Written};
use fastctx_types::observation::{MachineSource, Observation};
use fastctx_types::remote::RemoteCall;
use fastctx_types::{AccountName, MachineName};
use tokio::time::Instant;

use crate::Node;

/// How long an answer stands while its machine's number does: past it, the
/// machine is asked again, in case it moved without telling.
const TRUSTED: Duration = Duration::from_secs(300);
/// How long an answer stands from a machine that tells no number.
const FRESH: Duration = Duration::from_secs(15);
/// How long a machine that did not answer is told so before it is asked
/// again.
const FAILED: Duration = Duration::from_secs(10);
/// How long a reader that takes what is known at once waits for the
/// machines being asked.
const SOON: Duration = Duration::from_millis(1500);
/// How long each asking, and any other reader, waits for its machine.
const WAITED: Duration = Duration::from_secs(30);
/// The most machines asked at once, as other fan-outs ask
/// (`fastctx_net::around`).
const ASKING_AT_ONCE: usize = 32;

/// What is asked: each kind has its own answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Kind {
    Agents,
    Logins,
}

impl Kind {
    fn call(self) -> RemoteCall {
        match self {
            Self::Agents => RemoteCall::Agents,
            Self::Logins => RemoteCall::AgentLogins,
        }
    }
}

/// A machine asked for an account through a hub: the hub, the machine's
/// key, the account and when it was opened.
type Target = (Kind, HubId, String, AccountName, i64);

/// A machine, by its hub and key.
type Place = (HubId, String);

/// One asking under way, watched by whoever waits on it: its answer once
/// it came.
type Asking = tokio::sync::watch::Receiver<Option<Heard>>;

struct Kept {
    heard: Heard,
    at: Instant,
    /// The number the machine's agents and logins stood at as it was asked.
    number: Option<u64>,
    /// Something done here changed them there since it was asked: asked
    /// again before told.
    moved: bool,
    /// How its hub's view had it when last looked at: whether online, and
    /// its number.
    seen: (bool, Option<u64>),
}

/// The answers kept, the askings under way, and this machine's own number.
pub(crate) struct Answers {
    kept: Mutex<HashMap<Target, Kept>>,
    asking: Mutex<HashMap<Target, Asking>>,
    /// How many times something done here changed each machine's logins.
    moves: Mutex<HashMap<Place, u64>>,
    /// The machines each hub said were online when last looked at.
    online: Mutex<HashMap<HubId, HashSet<String>>>,
    turns: Arc<tokio::sync::Semaphore>,
    here: AtomicU64,
}

impl Default for Answers {
    fn default() -> Self {
        // Its own each time the node starts, so what another machine kept
        // from before is never taken for what stands now.
        let started = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| u64::try_from(since.as_micros()).unwrap_or(u64::MAX));
        Self {
            kept: Mutex::default(),
            asking: Mutex::default(),
            moves: Mutex::default(),
            online: Mutex::default(),
            turns: Arc::new(tokio::sync::Semaphore::new(ASKING_AT_ONCE)),
            here: AtomicU64::new(started),
        }
    }
}

/// How a reader reads what the machines say.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Reading {
    /// What stands at once; the others asked and waited on for `SOON`, one
    /// not answered by then told as last heard, or as being asked: the
    /// reader reads again once what it answers changed what was shown
    /// (`topics::LOGINS`).
    AtOnce,
    /// What stands at once; the others asked and waited on for `WAITED`.
    Waited,
    /// Each machine asked now and waited on for `WAITED`: the reader
    /// follows that machine's own topics, and reads again as they move.
    Fresh,
}

impl Reading {
    /// How `at_once` reads `machine`, or every machine with none.
    pub(crate) fn of(machine: Option<&Written>, at_once: bool) -> Self {
        match (machine, at_once) {
            (Some(_), _) => Self::Fresh,
            (None, true) => Self::AtOnce,
            (None, false) => Self::Waited,
        }
    }
}

/// What each of `reachable` says of `kind`, read as `reading` says. One not
/// answered in time is told, read at once, as last heard, or among the
/// second list when never heard from; read otherwise, as not answering.
pub(crate) async fn ask(
    node: &Arc<Node>,
    kind: Kind,
    reachable: Vec<Reachable>,
    reading: Reading,
) -> (Vec<(Asked, Heard)>, Vec<Asked>) {
    let now = Instant::now();
    let mut heard = Vec::with_capacity(reachable.len());
    let mut waiting = Vec::new();
    for target in reachable {
        let view = target.link.view();
        let seen = view.machines().find(|machine| machine.key == target.key).cloned();
        let opened = view.directory.account(&target.account).map_or(0, |account| account.opened.as_millisecond());
        drop(view);
        let mut written = target.written.clone();
        if let Some(seen) = &seen {
            written.name = seen.name.clone();
        }
        let asked = Asked {
            source: MachineSource { hub: target.link.membership().id.clone(), key: target.key.to_string(), written },
            account: target.account.clone(),
        };
        let seen = match seen {
            None => {
                heard.push((asked, Heard::NoAnswer("the hub did not say whether it is connected".to_string())));
                continue;
            }
            Some(seen) if !seen.online => {
                heard.push((asked, Heard::Offline(seen.since)));
                continue;
            }
            Some(seen) => seen,
        };
        let key = (kind, asked.source.hub.clone(), asked.source.key.clone(), asked.account.clone(), opened);
        let kept = node
            .elsewhere
            .kept
            .lock()
            .expect("never poisoned")
            .get(&key)
            .map(|kept| (kept.heard.clone(), reading != Reading::Fresh && standing(kept, seen.logins_number, now)));
        match kept {
            Some((answer, true)) => heard.push((asked, answer)),
            kept => {
                let underway = asking(node, kind, key, target, seen.logins_number);
                waiting.push((asked, kept.map(|(answer, _)| answer), underway));
            }
        }
    }
    let until = now + if reading == Reading::AtOnce { SOON } else { WAITED };
    let mut still = Vec::new();
    for (asked, kept, underway) in waiting {
        match (tokio::time::timeout_at(until, answered(underway)).await, kept) {
            (Ok(Some(answer)), _) => heard.push((asked, answer)),
            // Read at once, what was last heard stands meanwhile; read
            // otherwise, what was not answered now is not told as if it was.
            (_, Some(kept)) if reading == Reading::AtOnce => heard.push((asked, kept)),
            (_, None) if reading == Reading::AtOnce => still.push(asked),
            _ => heard.push((asked, Heard::NoAnswer(format!("it did not answer within {} seconds", WAITED.as_secs())))),
        }
    }
    heard.sort_by(|a, b| a.0.source.written.cmp(&b.0.source.written));
    still.sort_by(|a, b| a.source.written.cmp(&b.source.written));
    (heard, still)
}

/// What `ask` heard, one entry for each machine and in the order of their
/// names: each machine's answers as `told` puts them together, one being
/// asked as `being_asked` tells it.
pub(crate) fn listed<T>(
    heard: Vec<(Asked, Heard)>,
    asking: Vec<Asked>,
    told: impl Fn(fastctx_net::around::answers::Answers) -> T,
    being_asked: impl Fn(MachineName, Option<Observation>) -> T,
) -> Vec<T> {
    let groups = fastctx_net::around::answers::collect(heard);
    let mut waiting: Vec<Asked> = Vec::new();
    for asked in asking {
        let known = groups.iter().map(|group| &group.source).chain(waiting.iter().map(|asked| &asked.source));
        if !known.into_iter().any(|source| source.same_machine(&asked.source)) {
            waiting.push(asked);
        }
    }
    let mut listed: Vec<(MachineName, T)> = waiting
        .into_iter()
        .map(|asked| {
            let machine = asked.name().clone();
            let observation = Observation { source: asked.source, unanswered: Vec::new() };
            (machine.clone(), being_asked(machine, Some(observation)))
        })
        .collect();
    listed.extend(groups.into_iter().map(|answers| (answers.source.written.name.clone(), told(answers))));
    listed.sort_by(|a, b| a.0.cmp(&b.0));
    listed.into_iter().map(|(_, entry)| entry).collect()
}

/// Whether what `kept` says stands, its machine's number now `number`.
fn standing(kept: &Kept, number: Option<u64>, now: Instant) -> bool {
    let age = now.saturating_duration_since(kept.at);
    if kept.moved {
        return false;
    }
    match (&kept.heard, kept.number, number) {
        (Heard::NoAnswer(_), _, _) => age < FAILED,
        (_, Some(then), Some(number)) => then == number && age < TRUSTED,
        _ => age < FRESH,
    }
}

/// The asking of `target` under way, or begun now, in its turn among the
/// node's askings: what it answers is kept, and readers are told to read
/// again once it changed what they were shown.
fn asking(node: &Arc<Node>, kind: Kind, key: Target, target: Reachable, number: Option<u64>) -> Asking {
    let mut asking = node.elsewhere.asking.lock().expect("never poisoned");
    if let Some(underway) = asking.get(&key) {
        return underway.clone();
    }
    let (answer, underway) = tokio::sync::watch::channel(None);
    asking.insert(key.clone(), underway.clone());
    let place = (key.1.clone(), key.2.clone());
    let moves = node.elsewhere.moves.lock().expect("never poisoned").get(&place).copied().unwrap_or(0);
    let (weak, turns) = (Arc::downgrade(node), node.elsewhere.turns.clone());
    fastctx_platform::task::spawn(async move {
        let heard = match tokio::time::timeout(WAITED, turns.acquire_owned()).await {
            Ok(Ok(_turn)) => fastctx_net::around::ask_known(vec![target], |_| kind.call(), WAITED)
                .await
                .pop()
                .map_or_else(|| Heard::NoAnswer("it was not asked".to_string()), |(_, heard)| heard),
            _ => Heard::NoAnswer("it was not asked in time: too many were asked before it".to_string()),
        };
        let Some(node) = weak.upgrade() else { return };
        let changed = keep(&node, &key, &heard, number, moves);
        node.elsewhere.asking.lock().expect("never poisoned").remove(&key);
        let again = match &heard {
            Heard::NoAnswer(_) => Some(FAILED),
            Heard::Answered(_) if number.is_none() => Some(FRESH),
            _ => None,
        };
        answer.send_replace(Some(heard));
        if changed {
            node.topics.changed(fastctx_types::topics::LOGINS);
        }
        // One that did not answer, or tells no number, is read again once
        // what is kept of it stands no longer, by whoever still reads.
        if let Some(again) = again {
            drop(node);
            tokio::time::sleep(again + Duration::from_millis(100)).await;
            if let Some(node) = weak.upgrade() {
                node.topics.changed(fastctx_types::topics::LOGINS);
            }
        }
    });
    underway
}

/// What the asking watched answers, once it does.
async fn answered(mut underway: Asking) -> Option<Heard> {
    loop {
        if let Some(heard) = underway.borrow_and_update().clone() {
            return Some(heard);
        }
        if underway.changed().await.is_err() {
            return underway.borrow().clone();
        }
    }
}

/// Keeps what a machine said; one away is told as away by its hub, not
/// kept. Something done here to its logins since it was asked (`moves` then)
/// leaves it to be asked again. Whether it differs from what was kept.
fn keep(node: &Node, key: &Target, answer: &Heard, number: Option<u64>, moves: u64) -> bool {
    let place = (key.1.clone(), key.2.clone());
    let moved = node.elsewhere.moves.lock().expect("never poisoned").get(&place).copied().unwrap_or(0) != moves;
    let mut kept = node.elsewhere.kept.lock().expect("never poisoned");
    match answer {
        Heard::Offline(_) => kept.remove(key).is_some(),
        answer => {
            let changed = kept.get(key).is_none_or(|before| told(&before.heard) != told(answer));
            let kept_now = Kept { heard: answer.clone(), at: Instant::now(), number, moved, seen: (true, number) };
            kept.insert(key.clone(), kept_now);
            changed
        }
    }
}

/// What an answer tells, to see whether another tells the same.
fn told(heard: &Heard) -> Vec<u8> {
    match heard {
        Heard::Answered(reply) => serde_json::to_vec(reply).unwrap_or_default(),
        Heard::Offline(since) => format!("offline {since:?}").into_bytes(),
        Heard::NoAnswer(reason) => format!("no answer {reason}").into_bytes(),
    }
}

/// `link`'s directory changed, not only who is online: what each account
/// may see of each machine may have changed with it, so nothing kept from
/// its hub is told again before it is asked anew.
pub(crate) fn directory_changed(node: &Node, link: &Link) {
    let hub = link.membership().id;
    node.elsewhere.kept.lock().expect("never poisoned").retain(|(_, at, _, _, _), _| *at != hub);
    view_changed(node, link);
}

/// `link`'s view changed: whoever reads is told to read again when a machine
/// came or went, or the number of one kept here moved. What is kept of a
/// machine no longer in the view goes.
pub(crate) fn view_changed(node: &Node, link: &Link) {
    let hub = link.membership().id;
    let view = link.view();
    let standing: HashMap<String, (bool, Option<u64>)> =
        view.machines().map(|machine| (machine.key.to_string(), (machine.online, machine.logins_number))).collect();
    drop(view);
    let online: HashSet<String> =
        standing.iter().filter(|(_, (online, _))| *online).map(|(key, _)| key.clone()).collect();
    let mut moved = node
        .elsewhere
        .online
        .lock()
        .expect("never poisoned")
        .insert(hub.clone(), online.clone())
        .is_some_and(|before| before != online);
    let mut kept = node.elsewhere.kept.lock().expect("never poisoned");
    kept.retain(|(_, at, key, _, _), _| *at != hub || standing.contains_key(key));
    for ((_, at, key, _, _), kept) in kept.iter_mut() {
        if *at != hub {
            continue;
        }
        let now = standing.get(key).copied().unwrap_or((false, None));
        if kept.seen != now {
            kept.seen = now;
            moved = true;
        }
    }
    drop(kept);
    if moved {
        node.topics.changed(fastctx_types::topics::LOGINS);
    }
}

/// Something done here through `machine` changed its agents or logins: what
/// it said of them is asked again before it is told, whatever an asking
/// under way meanwhile brings.
pub(crate) fn moved(node: &Node, machine: &Written) {
    let Ok(links) = node.locate(machine) else { return };
    let places: Vec<Place> = links
        .iter()
        .filter_map(|link| {
            let key = link.view().member(&machine.name)?.key.to_string();
            Some((link.membership().id, key))
        })
        .collect();
    {
        let mut moves = node.elsewhere.moves.lock().expect("never poisoned");
        for place in &places {
            *moves.entry(place.clone()).or_default() += 1;
        }
    }
    for ((_, at, key, _, _), kept) in node.elsewhere.kept.lock().expect("never poisoned").iter_mut() {
        if places.iter().any(|(hub, of)| hub == at && of == key) {
            kept.moved = true;
        }
    }
}

/// What this machine answers of its agents and logins changed: its number
/// moves, and the hubs are told it with the next heartbeat. Not told at
/// once: a scan changes them every moment, and telling at each would keep
/// the heartbeat itself from going (codex-580b4d87).
pub(crate) fn changed_here(node: &Node) {
    node.elsewhere.here.fetch_add(1, Ordering::AcqRel);
}

/// The number this machine's agents and logins stand at, told the hubs with
/// each heartbeat.
pub(crate) fn here(node: &Node) -> u64 {
    node.elsewhere.here.load(Ordering::Acquire)
}
