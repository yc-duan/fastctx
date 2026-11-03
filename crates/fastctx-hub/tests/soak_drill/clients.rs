//! Who uses the node under test while it soaks: people at its own
//! interface typing, sending, stopping and opening; its interfaces
//! watching; phones watching from afar, coming and going, and asking too.
//! Every operation is timed where its user sits and has its outcome within
//! `OUTCOME`, or it is counted as one that never came
//! (`specs/world-foundation.md`, 宪法十一).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::future::Future;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fastctx_client::World;
use fastctx_types::access::Written;
use fastctx_types::composer::{Change, Mutation, Request, Target};
use fastctx_types::residents::Activity;
use fastctx_types::town::MachineWho;
use fastctx_types::{AccountName, AgentName, ResidentId};
use tokio::sync::watch;

use crate::setup::{SOAK, machine, reopen};

/// How long an operation may go before it is one that never came to an end.
pub(crate) const OUTCOME: Duration = Duration::from_secs(120);

/// One operation as its user saw it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Sample {
    pub at: Instant,
    pub kind: &'static str,
    pub took: Duration,
    pub ok: bool,
}

/// What the users saw.
#[derive(Default)]
pub(crate) struct Seen {
    samples: Mutex<Vec<Sample>>,
    /// Operations with no outcome within `OUTCOME`, by kind.
    hung: Mutex<BTreeMap<&'static str, u64>>,
    /// Failures by kind: how many, and the first said.
    failed: Mutex<BTreeMap<&'static str, (u64, String)>>,
    /// Watches that broke off other than by the drill's own doing.
    pub watches_broken: AtomicU64,
    pub watches_opened: AtomicU64,
}

impl Seen {
    /// Times `work`, an operation of `kind`, to its outcome.
    pub async fn time<T, E: std::fmt::Display>(
        &self,
        kind: &'static str,
        work: impl Future<Output = Result<T, E>>,
    ) -> Option<T> {
        let at = Instant::now();
        match tokio::time::timeout(OUTCOME, work).await {
            Err(_) => {
                *self.hung.lock().expect("never poisoned").entry(kind).or_default() += 1;
                None
            }
            Ok(result) => {
                let ok = result.is_ok();
                self.samples.lock().expect("never poisoned").push(Sample { at, kind, took: at.elapsed(), ok });
                match result {
                    Ok(value) => Some(value),
                    Err(error) => {
                        let mut failed = self.failed.lock().expect("never poisoned");
                        let entry = failed.entry(kind).or_insert_with(|| (0, error.to_string()));
                        entry.0 += 1;
                        None
                    }
                }
            }
        }
    }

    pub fn samples(&self) -> Vec<Sample> {
        self.samples.lock().expect("never poisoned").clone()
    }

    pub fn hung(&self) -> BTreeMap<&'static str, u64> {
        self.hung.lock().expect("never poisoned").clone()
    }

    pub fn failed(&self) -> BTreeMap<&'static str, (u64, String)> {
        self.failed.lock().expect("never poisoned").clone()
    }
}

/// A small generator of choices, its seed printed so a run can be told
/// again.
pub(crate) struct Dice(u64);

impl Dice {
    pub fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }

    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    /// A number below `n`.
    pub fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }

    pub fn seconds(&mut self, from: u64, to: u64) -> Duration {
        Duration::from_millis(from * 1000 + self.below((to - from) * 1000 + 1))
    }
}

static OPERATION: AtomicU64 = AtomicU64::new(0);

fn operation() -> String {
    format!("soak-{}-{}", std::process::id(), OPERATION.fetch_add(1, Ordering::Relaxed))
}

/// What each conversation of the node under test is doing, as its
/// interface is told, and which are spoken to now.
#[derive(Default)]
pub(crate) struct Conversations {
    now: Mutex<HashMap<ResidentId, Activity>>,
    known: Mutex<Vec<ResidentId>>,
    taken: Mutex<HashSet<ResidentId>>,
    changed: tokio::sync::Notify,
}

impl Conversations {
    pub fn add(&self, resident: ResidentId) {
        self.known.lock().expect("never poisoned").push(resident);
    }

    pub fn count(&self) -> usize {
        self.known.lock().expect("never poisoned").len()
    }

    /// One no one speaks to now, taken until `give` gives it back.
    pub fn take(&self, dice: &mut Dice) -> Option<ResidentId> {
        let known = self.known.lock().expect("never poisoned");
        let mut taken = self.taken.lock().expect("never poisoned");
        for _ in 0..8 {
            let resident = known.get(dice.below(known.len() as u64) as usize)?;
            if taken.insert(resident.clone()) {
                return Some(resident.clone());
            }
        }
        None
    }

    pub fn give(&self, resident: &ResidentId) {
        self.taken.lock().expect("never poisoned").remove(resident);
    }

    pub fn any(&self, dice: &mut Dice) -> Option<ResidentId> {
        let known = self.known.lock().expect("never poisoned");
        known.get(dice.below(known.len() as u64) as usize).cloned()
    }

    fn activity(&self, resident: &ResidentId) -> Option<Activity> {
        self.now.lock().expect("never poisoned").get(resident).copied()
    }

    /// Waits for `resident` to be at rest: given a moment to start working
    /// on what it was just told, then until it is done, at most `within`.
    pub async fn rest(&self, resident: &ResidentId, within: Duration) -> bool {
        let began = Instant::now();
        let started = Instant::now() + Duration::from_secs(2);
        loop {
            let activity = self.activity(resident);
            let busy = activity == Some(Activity::Busy);
            if !busy && Instant::now() >= started {
                return true;
            }
            if began.elapsed() >= within {
                return false;
            }
            let _ = tokio::time::timeout(Duration::from_millis(250), self.changed.notified()).await;
        }
    }

    /// Follows the node's World as its interface does, keeping what each
    /// conversation is doing, until `stop`.
    pub async fn follow(self: Arc<Self>, local: World, seen: Arc<Seen>, mut stop: watch::Receiver<bool>) {
        while !*stop.borrow() {
            let watching = tokio::select! {
                watching = local.watch_world() => watching,
                _ = stop.changed() => return,
            };
            let Ok(mut watching) = watching else {
                tokio::time::sleep(Duration::from_secs(1)).await;
                continue;
            };
            seen.watches_opened.fetch_add(1, Ordering::Relaxed);
            loop {
                let next = tokio::select! {
                    next = watching.next() => next,
                    _ = stop.changed() => return,
                };
                let Ok(world) = next else {
                    seen.watches_broken.fetch_add(1, Ordering::Relaxed);
                    break;
                };
                let mut now = HashMap::new();
                for answered in &world.who.machines {
                    let MachineWho::Answered { reply, .. } = answered else { continue };
                    if reply.machine.as_str() != SOAK {
                        continue;
                    }
                    for resident in &reply.residents {
                        let activity = resident.now.as_ref().map_or(Activity::Idle, |now| now.activity);
                        now.insert(resident.about.id.clone(), activity);
                    }
                }
                *self.now.lock().expect("never poisoned") = now;
                self.changed.notify_waiters();
            }
        }
    }
}

/// Types `text` into `resident`'s shared input a few words at a time, then
/// sends it, as a person at an interface does.
pub(crate) async fn type_and_send(
    world: &World,
    seen: &Seen,
    prefix: &'static [&'static str; 2],
    resident: &ResidentId,
    machine: Option<Written>,
    text: &str,
) -> bool {
    let Some(read) = seen.time(prefix[0], world.composer(resident.clone(), machine.clone(), Request::Read)).await
    else {
        return false;
    };
    let mut composer = read.composer;
    for words in text.split_inclusive(' ').collect::<Vec<_>>().chunks(3) {
        let mutation = Mutation {
            operation: operation(),
            change: Change::Edit {
                target: Target::Draft,
                epoch: composer.draft.epoch,
                after: composer.draft.characters.last().map(|character| character.id.clone()),
                delete: Vec::new(),
                insert: words.concat(),
            },
        };
        let Some(updated) =
            seen.time(prefix[0], world.update_composer(resident.clone(), machine.clone(), mutation)).await
        else {
            return false;
        };
        composer = updated.composer;
    }
    let send = Mutation { operation: operation(), change: Change::Send { epoch: composer.draft.epoch } };
    seen.time(prefix[1], world.update_composer(resident.clone(), machine, send)).await.is_some()
}

const LOCAL: [&str; 2] = ["type", "send"];
const PHONE: [&str; 2] = ["phone.type", "phone.send"];

/// What a person asks for, with the model mock's markers
/// (`scripts/model_mock.py`): mostly plain words, sometimes a command.
fn words(dice: &mut Dice, n: u64) -> String {
    match dice.below(10) {
        0..=5 => format!("soak words {n}: how are things going on this machine today"),
        6..=8 => format!("soak run {n} [[run: echo soak-{n}; sleep 1]]"),
        _ => format!("soak later {n} [[later: sleep 20; echo soak-late-{n}]]"),
    }
}

/// A person at the node's own interface, until `stop`.
pub(crate) async fn person(
    index: u64,
    local: World,
    conversations: Arc<Conversations>,
    seen: Arc<Seen>,
    open: Opening,
    mut stop: watch::Receiver<bool>,
) {
    let mut dice = Dice::new(seed() ^ (index + 1).wrapping_mul(0x9e37_79b9));
    let mut n = index * 1_000_000;
    while !*stop.borrow() {
        n += 1;
        let choice = dice.below(100);
        if choice < 8 {
            if let Some(resident) = open.open(&local, &seen, "open", n).await {
                conversations.add(resident);
            }
        } else if choice < 20 {
            let _ = seen.time("read", local.who(None, true)).await;
            let _ = seen.time("read", local.nodes()).await;
        } else if let Some(resident) = conversations.take(&mut dice) {
            if choice < 35 {
                // Something slow, stopped once it is under way.
                if type_and_send(&local, &seen, &LOCAL, &resident, None, &format!("soak slow {n} [[slow: 8]]")).await {
                    tokio::time::sleep(dice.seconds(1, 3)).await;
                    let _ = seen.time("stop", local.stop(resident.clone(), None)).await;
                }
            } else {
                let text = words(&mut dice, n);
                type_and_send(&local, &seen, &LOCAL, &resident, None, &text).await;
            }
            conversations.rest(&resident, OUTCOME).await;
            conversations.give(&resident);
        }
        tokio::select! {
            () = tokio::time::sleep(dice.seconds(0, 2)) => {},
            _ = stop.changed() => {},
        }
    }
}

/// How conversations are opened on the node under test.
#[derive(Clone)]
pub(crate) struct Opening {
    pub agent: AgentName,
    pub home: PathBuf,
}

impl Opening {
    pub async fn open(&self, world: &World, seen: &Seen, kind: &'static str, n: u64) -> Option<ResidentId> {
        let choices = fastctx_types::agents::Choices { home: Some(self.home.clone()), ..Default::default() };
        let said = seen
            .time(
                kind,
                world.open_with(
                    self.agent.clone(),
                    None,
                    Some(SOAK.parse().expect("a machine")),
                    format!("soak opening {n}"),
                    Default::default(),
                    choices,
                ),
            )
            .await?;
        Some(said.to.resident)
    }

    /// Opens one, trying again a while as a person would: a node just
    /// started may not take one yet. What it said, when it never did.
    pub async fn open_soon(&self, world: &World, seen: &Seen, n: u64) -> Result<ResidentId, String> {
        let began = Instant::now();
        loop {
            if let Some(resident) = self.open(world, seen, "open", n).await {
                return Ok(resident);
            }
            if began.elapsed() >= Duration::from_secs(180) {
                let said = seen.failed().get("open").map(|(_, said)| said.clone());
                return Err(said.unwrap_or_else(|| "no outcome came".into()));
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    }
}

/// One of the node's own interfaces: it watches the World and one
/// conversation for a while, then goes and comes back, until `stop`.
pub(crate) async fn interface(
    index: u64,
    local: World,
    conversations: Arc<Conversations>,
    seen: Arc<Seen>,
    mut stop: watch::Receiver<bool>,
) {
    let mut dice = Dice::new(seed() ^ (index + 7).wrapping_mul(0x85eb_ca6b));
    while !*stop.borrow() {
        let shown = dice.seconds(20, 90);
        watch_for(&local, &conversations, &seen, &mut dice, None, shown, &mut stop).await;
        tokio::select! {
            () = tokio::time::sleep(dice.seconds(1, 5)) => {},
            _ = stop.changed() => {},
        }
    }
}

/// Watches the World and one conversation for `shown`.
async fn watch_for(
    world: &World,
    conversations: &Conversations,
    seen: &Seen,
    dice: &mut Dice,
    resident: Option<ResidentId>,
    shown: Duration,
    stop: &mut watch::Receiver<bool>,
) {
    let until = tokio::time::Instant::now() + shown;
    let Some(mut whole) = seen.time("watch", world.watch_world()).await else { return };
    let resident = resident.or_else(|| conversations.any(dice));
    let mut one = match resident {
        Some(resident) => seen.time("watch", world.watch(resident, Some(machine(SOAK).into()))).await,
        None => None,
    };
    seen.watches_opened.fetch_add(1, Ordering::Relaxed);
    loop {
        tokio::select! {
            () = tokio::time::sleep_until(until) => return,
            _ = stop.changed() => return,
            next = whole.next_seen() => if next.is_err() {
                seen.watches_broken.fetch_add(1, Ordering::Relaxed);
                return;
            },
            next = async { match one.as_mut() { Some(one) => one.next().await, None => std::future::pending().await } } => {
                if !matches!(next, Ok(Some(_))) {
                    one = None;
                }
            },
        }
    }
}

/// A phone, opened again from what it keeps each time it comes back: it
/// watches, and every third one types and sends into a conversation of its
/// account, until `stop`.
pub(crate) async fn phone(
    index: u64,
    directory: PathBuf,
    owner: AccountName,
    conversations: Arc<Conversations>,
    seen: Arc<Seen>,
    open: Opening,
    mut stop: watch::Receiver<bool>,
) {
    let mut dice = Dice::new(seed() ^ (index + 13).wrapping_mul(0xc2b2_ae35));
    let speaks = index.is_multiple_of(3);
    // Bob's conversations are his own, opened through the login alice
    // lets him use.
    let mut own: Option<ResidentId> = None;
    let mut n = 1_000_000_000 + index * 1_000_000;
    while !*stop.borrow() {
        let Ok((device, world)) = reopen(&directory) else {
            tokio::time::sleep(Duration::from_secs(1)).await;
            continue;
        };
        if owner.as_str() == "bob" && own.is_none() {
            own = open.open(&world, &seen, "phone.open", n).await;
        }
        let resident = if owner.as_str() == "bob" { own.clone() } else { conversations.any(&mut dice) };
        let shown = dice.seconds(20, 90);
        let until = tokio::time::Instant::now() + shown;
        let watching = watch_for(&world, &conversations, &seen, &mut dice, resident.clone(), shown, &mut stop);
        let speaking = async {
            while speaks && tokio::time::Instant::now() < until {
                tokio::time::sleep(dice_seconds(index, 8, 15)).await;
                if let Some(resident) = &resident {
                    n += 1;
                    type_and_send(
                        &world,
                        &seen,
                        &PHONE,
                        resident,
                        Some(machine(SOAK).into()),
                        &format!("soak phone {n}: a word from afar"),
                    )
                    .await;
                }
            }
            std::future::pending::<()>().await
        };
        tokio::select! {
            () = watching => {},
            () = speaking => {},
        }
        device.disconnect();
        drop(world);
        drop(device);
        tokio::select! {
            () = tokio::time::sleep(dice.seconds(1, 5)) => {},
            _ = stop.changed() => {},
        }
    }
}

/// A pause of `from` to `to` seconds for a phone's own pace, apart from
/// its dice, which its watch borrows meanwhile.
fn dice_seconds(index: u64, from: u64, to: u64) -> Duration {
    let mut dice = Dice::new(seed() ^ index ^ OPERATION.load(Ordering::Relaxed));
    dice.seconds(from, to)
}

/// The run's seed, from the clock unless `SOAK_SEED` says.
pub(crate) fn seed() -> u64 {
    static SEED: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *SEED.get_or_init(|| {
        std::env::var("SOAK_SEED").ok().and_then(|seed| seed.parse().ok()).unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(1, |since| since.as_nanos() as u64)
        })
    })
}
