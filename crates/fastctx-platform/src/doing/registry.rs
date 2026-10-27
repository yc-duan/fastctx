//! The one record of every ranked lock held or waited for and every wait on
//! something outside the process, with what its task was doing.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, OnceLock};
use std::time::{Duration, Instant};

use super::Rank;

static NEXT: AtomicU64 = AtomicU64::new(1);
static STATE: LazyLock<Mutex<State>> = LazyLock::new(Mutex::default);
/// Where a lock taken out of order is told.
type Reporter = Box<dyn Fn(&str) + Send + Sync>;

static REPORTER: OnceLock<Reporter> = OnceLock::new();
static VIOLATIONS: AtomicU64 = AtomicU64::new(0);

pub(super) fn next() -> u64 {
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// Where what goes wrong with the order of locks is told: the node's log.
/// Until it is set, it goes to standard error.
pub fn set_reporter(report: impl Fn(&str) + Send + Sync + 'static) {
    let _ = REPORTER.set(Box::new(report));
}

/// How many times a task or thread took a lock out of order since the process
/// began.
pub fn violations() -> u64 {
    VIOLATIONS.load(Ordering::Relaxed)
}

pub(super) fn report(line: &str) {
    match REPORTER.get() {
        Some(report) => report(line),
        None => eprintln!("{line}"),
    }
}

/// Who holds or waits for a lock: a task, or a thread that is in none.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Who {
    Task(tokio::task::Id),
    Thread(std::thread::ThreadId),
}

#[derive(Default)]
struct State {
    held: HashMap<u64, Entry>,
    waiting: HashMap<u64, Entry>,
    outside: HashMap<u64, Outside>,
    active: HashMap<u64, Active>,
    /// Out-of-order pairs already told, so each is told once.
    told: HashSet<(&'static str, &'static str)>,
}

#[derive(Clone)]
struct Entry {
    lock: u64,
    who: Option<Who>,
    rank: Rank,
    name: &'static str,
    subject: Arc<str>,
    /// Held or wanted alone: a writer, or a mutex.
    alone: bool,
    label: Arc<str>,
    since: Instant,
    progress: Instant,
}

struct Outside {
    task: Option<tokio::task::Id>,
    what: String,
    within: Duration,
    label: Arc<str>,
    since: Instant,
}

fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A lock this registry knows of, as one of its acquisitions names it.
pub(super) struct Lock<'a> {
    pub id: u64,
    pub rank: Rank,
    pub name: &'static str,
    pub subject: &'a Arc<str>,
}

/// A task waiting for a lock; it holds the lock once `held` is called, and
/// stops waiting when dropped first, as when its future is dropped.
pub(super) struct Wait {
    token: u64,
}

impl Wait {
    /// Starts a task's wait for `lock`, checking it against the locks this
    /// task already holds. Outside a task it is not checked: futures run
    /// together on one thread by `block_on` are not one holder.
    pub fn begin(lock: Lock<'_>, alone: bool) -> Self {
        Self::start(lock, alone, tokio::task::try_id().map(Who::Task))
    }

    /// Starts a wait that blocks its thread until `lock` is taken alone,
    /// checked against what its task holds, or, outside a task, its thread.
    pub fn begin_blocking(lock: Lock<'_>) -> Self {
        let who = tokio::task::try_id().map_or_else(|| Who::Thread(std::thread::current().id()), Who::Task);
        Self::start(lock, true, Some(who))
    }

    fn start(lock: Lock<'_>, alone: bool, who: Option<Who>) -> Self {
        let label = super::label();
        let token = next();
        let mut state = state();
        if let Some(who) = who {
            // Locks of one place are taken in the order of their subjects,
            // as when one task takes those of several sessions.
            let late = state
                .held
                .values()
                .filter(|held| {
                    held.who == Some(who)
                        && (held.rank > lock.rank || (held.rank == lock.rank && *held.subject >= **lock.subject))
                })
                .max_by_key(|held| held.rank)
                .cloned();
            if let Some(held) = late {
                VIOLATIONS.fetch_add(1, Ordering::Relaxed);
                if state.told.insert((held.name, lock.name)) {
                    let line = format!(
                        "lock order: {label} took {}({}) while holding {}({}), which comes at or after it",
                        lock.name, lock.subject, held.name, held.subject
                    );
                    drop(state);
                    report(&line);
                    state = self::state();
                }
            }
        }
        state.waiting.insert(
            token,
            Entry {
                lock: lock.id,
                who,
                rank: lock.rank,
                name: lock.name,
                subject: lock.subject.clone(),
                alone,
                label,
                since: Instant::now(),
                progress: Instant::now(),
            },
        );
        Self { token }
    }

    /// The lock is taken: what was a wait is now held, until the returned
    /// hold is dropped.
    pub fn held(self) -> Hold {
        let mut state = state();
        let token = self.token;
        std::mem::forget(self);
        if let Some(mut entry) = state.waiting.remove(&token) {
            entry.since = Instant::now();
            entry.progress = entry.since;
            state.held.insert(token, entry);
        }
        Hold { token }
    }
}

impl Drop for Wait {
    fn drop(&mut self) {
        state().waiting.remove(&self.token);
    }
}

/// A lock taken without waiting (`try_lock`): held, never checked for order,
/// as taking it cannot wait on anyone.
pub(super) fn taken(lock: Lock<'_>, alone: bool) -> Hold {
    let token = next();
    state().held.insert(
        token,
        Entry {
            lock: lock.id,
            who: tokio::task::try_id().map(Who::Task),
            rank: lock.rank,
            name: lock.name,
            subject: lock.subject.clone(),
            alone,
            label: super::label(),
            since: Instant::now(),
            progress: Instant::now(),
        },
    );
    Hold { token }
}

/// What the task keeping others from `lock` is doing: one holding it alone,
/// else one waiting to hold it alone, which those after it wait behind, else
/// one holding a share of it.
pub(super) fn keeping(lock: u64) -> Option<Arc<str>> {
    let state = state();
    let of = |entries: &HashMap<u64, Entry>, alone: bool| {
        entries
            .values()
            .filter(|entry| entry.lock == lock && entry.alone == alone)
            .min_by_key(|entry| entry.since)
            .map(|entry| entry.label.clone())
    };
    of(&state.held, true).or_else(|| of(&state.waiting, true)).or_else(|| of(&state.held, false))
}

/// A ranked lock held; dropping it lets the registry forget it.
pub(super) struct Hold {
    token: u64,
}

impl Hold {
    pub(super) fn adopt(&self) {
        let label = super::label();
        if let Some(held) = state().held.get_mut(&self.token) {
            held.who = tokio::task::try_id().map(Who::Task);
            held.label = label;
        }
    }
}

impl Drop for Hold {
    fn drop(&mut self) {
        state().held.remove(&self.token);
    }
}

/// A unit of work that prevents handover, including work without a total deadline.
/// It is diagnostic state, not a mutex or an artificial time limit on long work.
#[derive(Clone, Debug)]
pub struct Active {
    pub what: String,
    pub doing: String,
    pub since: Instant,
}

pub struct Work {
    token: u64,
}

impl Work {
    pub fn begin(what: impl Into<String>) -> Self {
        let token = next();
        state()
            .active
            .insert(token, Active { what: what.into(), doing: super::label().to_string(), since: Instant::now() });
        Self { token }
    }
}

impl Drop for Work {
    fn drop(&mut self) {
        state().active.remove(&self.token);
    }
}

/// Reports completed application work by this task. Transport heartbeats must
/// never call this: only actual bytes or a completed local write count.
pub fn progress() {
    let Some(who) = tokio::task::try_id().map(Who::Task) else { return };
    let mut state = state();
    let now = Instant::now();
    for entry in state.held.values_mut().filter(|entry| entry.who == Some(who)) {
        entry.progress = now;
    }
}

/// A wait on something outside the process, kept while it lasts.
pub struct OutsideWait {
    token: u64,
}

impl OutsideWait {
    pub(super) fn begin(what: String, within: Duration) -> Self {
        let token = next();
        state().outside.insert(
            token,
            Outside { task: tokio::task::try_id(), what, within, label: super::label(), since: Instant::now() },
        );
        Self { token }
    }
}

impl Drop for OutsideWait {
    fn drop(&mut self) {
        state().outside.remove(&self.token);
    }
}

/// Something that has gone on too long: a task waiting for a lock past
/// `waits`, a lock held past `holds`, or a wait outside past its deadline.
#[derive(Clone, Debug)]
pub struct Stuck {
    /// The same for as long as the same task waits for, or holds, the same
    /// thing: to tell each once.
    pub key: String,
    /// What its task was doing.
    pub doing: String,
    /// What it waits for, or holds, in words for a log line.
    pub what: String,
    pub since: Duration,
}

/// Tasks that wait on each other in a circle: none of them can go on.
#[derive(Clone, Debug)]
pub struct Cycle {
    /// Each task in the circle: what it does and the lock it waits for.
    pub links: Vec<String>,
}

/// What the registry shows at one moment.
pub struct Snapshot {
    pub stuck: Vec<Stuck>,
    pub cycles: Vec<Cycle>,
    pub active: Vec<Active>,
}

/// Current task waits, including those that have not become late. A single
/// pass lets a caller registry join diagnostics without scanning per call.
pub fn task_waits() -> HashMap<tokio::task::Id, String> {
    waits_of(&state())
}

/// A final diagnostic never waits for a possibly stalled registry owner.
pub fn try_task_waits() -> Option<HashMap<tokio::task::Id, String>> {
    STATE.try_lock().ok().map(|state| waits_of(&state))
}

fn waits_of(state: &State) -> HashMap<tokio::task::Id, String> {
    let mut waits: HashMap<_, _> = state
        .waiting
        .values()
        .filter_map(|entry| {
            let Some(Who::Task(task)) = entry.who else { return None };
            Some((task, format!("waiting for {}({})", entry.name, entry.subject)))
        })
        .collect();
    for outside in state.outside.values() {
        if let Some(task) = outside.task {
            waits.insert(task, format!("waiting for {}", outside.what));
        }
    }
    waits
}

/// What has gone on too long and who waits on whom in a circle, now.
pub fn snapshot(waits: Duration, holds: Duration) -> Snapshot {
    let state = state();
    let now = Instant::now();
    let mut stuck = Vec::new();
    for wait in state.waiting.values() {
        let since = now.duration_since(wait.since);
        if since < waits {
            continue;
        }
        let holders: Vec<String> = state
            .held
            .values()
            .filter(|held| held.lock == wait.lock)
            .map(|held| format!("{} since {} s ago", held.label, now.duration_since(held.since).as_secs()))
            .collect();
        let held = if holders.is_empty() { "no one holds it now".to_string() } else { holders.join("; ") };
        stuck.push(Stuck {
            key: format!("waits {} {}({})", wait.label, wait.name, wait.subject),
            doing: wait.label.to_string(),
            what: format!("waits for {}({}), held by {held}", wait.name, wait.subject),
            since,
        });
    }
    for held in state.held.values() {
        let since = now.duration_since(held.progress);
        if since >= holds {
            stuck.push(Stuck {
                key: format!("holds {} {}({})", held.label, held.name, held.subject),
                doing: held.label.to_string(),
                what: format!("holds {}({}) without progress", held.name, held.subject),
                since,
            });
        }
    }
    for outside in state.outside.values() {
        let since = now.duration_since(outside.since);
        if since > outside.within + Duration::from_secs(5) {
            stuck.push(Stuck {
                key: format!("outside {} {}", outside.label, outside.what),
                doing: outside.label.to_string(),
                what: format!("waits for {} past its {} s", outside.what, outside.within.as_secs()),
                since,
            });
        }
    }
    let cycles = cycles(&state);
    Snapshot { stuck, cycles, active: state.active.values().cloned().collect() }
}

/// The circles of tasks and threads each waiting for a lock another of them
/// holds. A writer waits for every holder; a reader waits for a writer
/// holding the lock and for writers queued for it, which go first.
fn cycles(state: &State) -> Vec<Cycle> {
    let mut waits_on: HashMap<Who, (Vec<Who>, &Entry)> = HashMap::new();
    for wait in state.waiting.values() {
        let Some(who) = wait.who else { continue };
        let holders = state
            .held
            .values()
            .filter(|held| held.lock == wait.lock && (wait.alone || held.alone))
            .filter_map(|held| held.who);
        let queued = state
            .waiting
            .values()
            .filter(|queued| !wait.alone && queued.lock == wait.lock && queued.alone && queued.since < wait.since)
            .filter_map(|queued| queued.who);
        let blockers: Vec<_> = holders.chain(queued).filter(|blocker| *blocker != who).collect();
        waits_on.insert(who, (blockers, wait));
    }
    let mut found = Vec::new();
    let mut seen: HashSet<Who> = HashSet::new();
    for start in waits_on.keys() {
        if seen.contains(start) {
            continue;
        }
        let mut path: Vec<Who> = Vec::new();
        let mut on_path: HashSet<Who> = HashSet::new();
        walk(*start, &waits_on, &mut path, &mut on_path, &mut seen, &mut found);
    }
    found
}

fn walk(
    who: Who,
    waits_on: &HashMap<Who, (Vec<Who>, &Entry)>,
    path: &mut Vec<Who>,
    on_path: &mut HashSet<Who>,
    seen: &mut HashSet<Who>,
    found: &mut Vec<Cycle>,
) {
    if on_path.contains(&who) {
        let at = path.iter().position(|step| *step == who).unwrap_or(0);
        let links = path[at..]
            .iter()
            .filter_map(|step| waits_on.get(step))
            .map(|(_, wait)| format!("{} waits for {}({})", wait.label, wait.name, wait.subject))
            .collect();
        found.push(Cycle { links });
        return;
    }
    if !seen.insert(who) {
        return;
    }
    let Some((blockers, _)) = waits_on.get(&who) else { return };
    path.push(who);
    on_path.insert(who);
    for blocker in blockers {
        walk(*blocker, waits_on, path, on_path, seen, found);
    }
    on_path.remove(&who);
    path.pop();
}
