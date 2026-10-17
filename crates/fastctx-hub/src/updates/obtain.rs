//! A package a machine asks the hub for: from the hub's own copy; else
//! brought in once, however many ask (`world-network.md`, 中枢替网络存的东西),
//! from where its release published it, by the hub itself or by a machine
//! that holds it or reaches GitHub and fetches for others, the fastest
//! first. Every source is judged the same way (`fastctx_update::pace`):
//! while none going looks like finishing soon, the next is asked beside
//! them, a few at once, and the first to have the whole package, checked,
//! puts it in place.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use fastctx_net::wire::{Down, Told};
use fastctx_types::MachineName;
use fastctx_types::update::{Fetch, Held, Manifest, Package};
use fastctx_update::pace::{AT_ONCE, Pace, SAMPLE, Verdict};
use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;

use super::{Attempt, Fetching, Race};
use crate::{Hub, Out};

/// How long a machine asked to fetch a package may take before its first
/// byte comes, fetching it from GitHub meanwhile, and still look promising.
const MACHINE_START: Duration = Duration::from_secs(90);

/// How long bringing one package in may take in all, however it is
/// brought, giving its last try up included: within the 45 minutes whoever
/// asks for it waits (`fastctx_net::link`), so that it hears why.
const BUDGET: Duration = Duration::from_secs(43 * 60);

/// How long a try given up has to end its transfer before it is revoked.
const SETTLE: Duration = Duration::from_secs(30);

/// `asker` asks, as its question `id`, for the package `target` of
/// `version`: answered once the hub holds it, or with why it cannot.
pub(crate) fn ask(hub: &Arc<Hub>, id: u64, version: String, target: String, out: crate::outq::Outq) {
    let hub = hub.clone();
    tokio::spawn(async move {
        let answer = obtain(&hub, Held { version, target }).await.map(|size| Told::Package { size });
        let _ = out.send(Out::Frame(Down::Told { id, answer }));
    });
}

/// Brings the package `target` of `version` in with nobody waiting for it:
/// whoever asks next finds it held, or joins the bringing in still going.
pub(crate) fn bring_in(hub: &Arc<Hub>, version: String, target: String) {
    let hub = hub.clone();
    tokio::spawn(async move {
        // Why it could not is in the hub's log already.
        let _ = obtain(&hub, Held { version, target }).await;
    });
}

/// `name` could not fetch the package of the request `id`.
pub(crate) fn unfetched(hub: &Hub, name: &MachineName, id: u64, why: String) {
    let mut updates = hub.updates.lock().expect("never poisoned");
    if let Some(fetching) = updates.fetches.get_mut(&id).filter(|fetching| fetching.helper == *name)
        && let Some(done) = fetching.done.take()
    {
        let _ = done.send(Err(why));
    }
}

pub(super) struct Obtaining {
    waiters: Vec<oneshot::Sender<Result<u64, String>>>,
    expired: bool,
    id: u64,
}

/// The package's size once the hub holds it.
async fn obtain(hub: &Arc<Hub>, held: Held) -> Result<u64, String> {
    let (waiting, answered) = oneshot::channel();
    {
        let mut updates = hub.updates.lock().expect("never poisoned");
        updates.asked.insert(held.version.clone(), std::time::Instant::now());

        match updates.obtaining.get_mut(&held) {
            Some(owner) if owner.expired => {
                return Err(
                    "the previous package request timed out and its owner is still finishing; try again later".into()
                );
            }
            Some(owner) => {
                owner.waiters.retain(|waiter| !waiter.is_closed());
                if owner.waiters.len() >= 512 {
                    return Err("too many requests are waiting for this package; try again later".into());
                }
                owner.waiters.push(waiting);
            }
            None => {
                let id = hub.next();
                updates.obtaining.insert(held.clone(), Obtaining { waiters: vec![waiting], expired: false, id });
                tokio::spawn(own(hub.clone(), held.clone(), id));
            }
        }
    }
    answered.await.map_err(|_| "the hub stopped bringing it in".to_string())?
}

/// The deadline answers waiters; it never drops the actual package owner.
async fn own(hub: Arc<Hub>, held: Held, id: u64) {
    // The timer remains runnable even while the owner enters synchronous
    // publication I/O. Its identity cannot expire a subsequent owner.
    let timer = tokio::spawn(expire(hub.clone(), held.clone(), id));
    let brought = bring(&hub, &held).await;
    timer.abort();
    if let Err(why) = &brought {
        hub.log.line(format_args!("bringing in {} of {}: {why}", held.target, held.version));
    }
    let owner = hub.updates.lock().expect("never poisoned").obtaining.remove(&held);
    for waiter in owner.into_iter().flat_map(|owner| owner.waiters) {
        let _ = waiter.send(brought.clone());
    }
}

async fn expire(hub: Arc<Hub>, held: Held, id: u64) {
    tokio::time::sleep(BUDGET + 2 * SETTLE).await;
    let waiters = {
        let mut updates = hub.updates.lock().expect("never poisoned");
        let Some(owner) = updates.obtaining.get_mut(&held).filter(|owner| owner.id == id) else { return };
        owner.expired = true;
        std::mem::take(&mut owner.waiters)
    };
    let reason = format!(
        "bringing it in took more than {} minutes; its owner is still finishing",
        (BUDGET + 2 * SETTLE).as_secs() / 60
    );
    for waiter in waiters {
        let _ = waiter.send(Err(reason.clone()));
    }
}

fn size_of(cache: &fastctx_update::Cache, held: &Held) -> Result<Option<u64>, String> {
    cache.path(&held.version, &held.target)?;
    Ok(cache.get(&held.version, &held.target).and_then(|path| std::fs::metadata(path).ok()).map(|meta| meta.len()))
}

async fn bring(hub: &Arc<Hub>, held: &Held) -> Result<u64, String> {
    let (manifest, cache) = {
        let updates = hub.updates.lock().expect("never poisoned");
        (updates.manifest(&held.version), updates.cache.clone())
    };
    if let Some(size) = size_of(&cache, held)? {
        return Ok(size);
    }
    // One placed by hand is handed out from where it is.
    let placing = (hub.clone(), held.clone());
    if let Ok(Some((_, size))) = tokio::task::spawn_blocking(move || super::placed(&placing.0, &placing.1)).await {
        return Ok(size);
    }
    let manifest = manifest.ok_or_else(|| format!("no machine of the network knows the release {}", held.version))?;
    let package = manifest
        .package(&held.target)
        .cloned()
        .ok_or_else(|| format!("the release {} has no package for {}", held.version, held.target))?;
    let deadline = Instant::now() + BUDGET;
    let race = Arc::new(Race::default());
    let (ended, mut endings) = mpsc::unbounded_channel();
    let mut going: Vec<Going> = Vec::new();
    let mut tried = Tried { hub: package.url.is_none(), machines: Vec::new() };
    let mut failures = Vec::new();
    let mut look = tokio::time::interval(Duration::from_secs(1));
    let size = loop {
        if let Some(size) = size_of(&cache, held)? {
            break Some(size);
        }
        let now = Instant::now();
        if now >= deadline {
            failures.push(format!("it did not come within {} minutes in all", BUDGET.as_secs() / 60));
            break None;
        }
        let verdicts: Vec<Verdict> = going
            .iter_mut()
            .map(|try_| {
                try_.pace.heard(now.into_std(), try_.attempt.brought.load(Ordering::Relaxed));
                try_.pace.verdict(now.into_std(), package.size, try_.before_first)
            })
            .collect();
        for (index, verdict) in verdicts.iter().enumerate().rev() {
            if *verdict == Verdict::Stopped {
                let stopped = going.remove(index);
                failures.push(format!("{}: it brought nothing more for half a minute", stopped.source));
                settle(hub, &cache, held, stopped, deadline).await;
            }
        }
        let promising = verdicts.contains(&Verdict::Promising);
        if !promising && going.len() < AT_ONCE {
            match next(hub, held, &tried) {
                Some(Source::Hub) => {
                    tried.hub = true;
                    going.push(by_the_hub(hub, &cache, &manifest, held, &package, &race, &ended));
                }
                Some(Source::Machine(helper, generation)) => {
                    tried.machines.push(helper.clone());
                    going.push(by_a_machine(hub, held, &package, helper, generation, &race, &ended));
                }
                None if going.is_empty() => break None,
                None => {}
            }
        }
        tokio::select! {
            Some((id, outcome)) = endings.recv() => {
                if let Some(index) = going.iter().position(|try_| try_.attempt.id == id) {
                    let over = going.remove(index);
                    if let Err(why) = outcome {
                        failures.push(format!("{}: {why}", over.source));
                    }
                    settle(hub, &cache, held, over, deadline).await;
                }
            }
            _ = look.tick() => {}
        }
    };
    if let Some(size) = size {
        // Nobody waits for the others to stop once the package is held.
        let (hub, cache, held) = (hub.clone(), cache.clone(), held.clone());
        tokio::spawn(async move {
            for try_ in going {
                settle(&hub, &cache, &held, try_, deadline).await;
            }
        });
        return Ok(size);
    }
    for try_ in going {
        settle(hub, &cache, held, try_, deadline).await;
    }
    // Whatever was said, what one brought may be in place now.
    if let Some(size) = size_of(&cache, held)? {
        return Ok(size);
    }
    if failures.is_empty() {
        return Err("no machine of the network can reach GitHub or holds the package now".to_string());
    }
    Err(failures.join("; "))
}

/// Where a try brings the package from.
#[derive(Clone, Debug)]
enum Source {
    /// The hub downloads it from where its release published it.
    Hub,
    /// A machine, in its session of this generation, fetches or hands it.
    Machine(MachineName, u64),
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Hub => f.write_str("the hub's own download"),
            Self::Machine(name, _) => write!(f, "{name}"),
        }
    }
}

/// What was asked already: the hub's own download, and which machines.
struct Tried {
    hub: bool,
    machines: Vec<MachineName>,
}

/// One try going, and how it goes.
struct Going {
    source: Source,
    attempt: Arc<Attempt>,
    pace: Pace,
    /// How long it may take before its first byte.
    before_first: Duration,
}

/// The hub's own download of the package, as one try: its bytes counted as
/// they come, stopped when the try is given up, and put in place only if
/// no other try put the package there first. How fast it went is kept, to
/// weigh the hub against its machines next time.
fn by_the_hub(
    hub: &Arc<Hub>,
    cache: &fastctx_update::Cache,
    manifest: &Manifest,
    held: &Held,
    package: &Package,
    race: &Arc<Race>,
    ended: &mpsc::UnboundedSender<(u64, Result<(), String>)>,
) -> Going {
    let id = hub.next();
    let attempt = Arc::new(Attempt::new(id, race.clone()));
    attempt.begin();
    let url = package.url.clone().expect("the hub downloads only a package published somewhere");
    let (hub, cache, manifest, held, ended) =
        (hub.clone(), cache.clone(), manifest.clone(), held.clone(), ended.clone());
    let trying = attempt.clone();
    tokio::spawn(async move {
        let outcome = async {
            let incoming = cache.attempt(&held.version, &held.target, id)?;
            let counting = trying.clone();
            let mut heard = move |brought, _| counting.brought.store(brought, Ordering::Relaxed);
            let speed = tokio::select! {
                downloaded = fastctx_update::download::patiently(&url, &incoming, &mut heard) => downloaded?,
                () = trying.stopping() => return Err("given up for another".to_string()),
            };
            if let Some(speed) = speed {
                hub.updates.lock().expect("never poisoned").own_speed = Some(speed);
            }
            if !trying.keep() {
                return Err("given up for another".to_string());
            }
            super::transfer::take_in(&hub, &cache, &manifest, &held.target, &incoming, || trying.commit())
                .await
                .map(|_| ())
        }
        .await;
        trying.over();
        let _ = ended.send((id, outcome));
    });
    Going { source: Source::Hub, attempt, pace: Pace::new(std::time::Instant::now()), before_first: SAMPLE }
}

/// A machine asked to fetch the package, or hand the one it holds, as one
/// try: its transfer counts the bytes (`super::transfer`).
fn by_a_machine(
    hub: &Arc<Hub>,
    held: &Held,
    package: &Package,
    helper: MachineName,
    generation: u64,
    race: &Arc<Race>,
    ended: &mpsc::UnboundedSender<(u64, Result<(), String>)>,
) -> Going {
    let id = hub.next();
    let attempt = Arc::new(Attempt::new(id, race.clone()));
    let (done, delivered) = oneshot::channel();
    let fetching =
        Fetching { helper: helper.clone(), generation, held: held.clone(), done: Some(done), attempt: attempt.clone() };
    hub.updates.lock().expect("never poisoned").fetches.insert(id, fetching);
    let fetch = Fetch { id, version: held.version.clone(), target: held.target.clone(), package: package.clone() };
    let sent = hub
        .sessions()
        .get(&helper)
        .filter(|session| session.generation == generation)
        .is_some_and(|session| session.out.send(Out::Frame(Down::Fetch { fetch: Box::new(fetch) })).is_ok());
    let ended = ended.clone();
    if sent {
        hub.log.line(format_args!("asked {helper} to fetch {} of {}", held.target, held.version));
        tokio::spawn(async move {
            let outcome = delivered.await.unwrap_or_else(|_| Err("the request was dropped".to_string()));
            let _ = ended.send((id, outcome));
        });
    } else {
        let _ = ended.send((id, Err("it left before it was asked".to_string())));
    }
    Going {
        source: Source::Machine(helper, generation),
        attempt,
        pace: Pace::new(std::time::Instant::now()),
        before_first: MACHINE_START,
    }
}

/// Ends a try: its transfer is told to stop and given a moment to, and
/// what it left is deleted. One with the whole package checks it and puts
/// it in place, if no other did, within what is left of the budget; one
/// that does not end in time can no longer put its package in place, and
/// deletes what it left itself; one putting it in place then finishes.
async fn settle(hub: &Hub, cache: &fastctx_update::Cache, held: &Held, try_: Going, deadline: Instant) {
    let Going { source, attempt, .. } = try_;
    attempt.give_up();
    let settle = match attempt.keeping() {
        true => deadline.max(Instant::now() + SETTLE),
        false => Instant::now() + SETTLE,
    };
    if tokio::time::timeout_at(settle, attempt.ended()).await.is_err() {
        hub.log.line(format_args!(
            "the transfer from {source} did not end in time; it may no longer put its package in place"
        ));
        attempt.revoke();
        attempt.ended().await;
    }
    hub.updates.lock().expect("never poisoned").fetches.remove(&attempt.id);
    if attempt.is_over()
        && let Ok(left) = cache.attempt(&held.version, &held.target, attempt.id)
    {
        super::cleanup::temporary(hub, &left).await;
    }
}

/// The next source to try, not one tried: a machine that holds the package
/// first; then, of the hub's own download and the machines that reached
/// GitHub and fetch for others, the fastest at its last download from
/// GitHub, those of unknown speed after, the hub before a machine of the
/// same.
fn next(hub: &Hub, held: &Held, tried: &Tried) -> Option<Source> {
    let sessions = hub.sessions();
    let updates = hub.updates.lock().expect("never poisoned");
    let online = |name: &MachineName, generation: u64| {
        sessions.get(name).is_some_and(|session| session.versions && session.generation == generation)
    };
    let candidates = updates
        .reports
        .iter()
        .filter(|(name, (generation, _))| !tried.machines.contains(name) && online(name, *generation));
    // Holders first, then the fastest; a known speed before an unknown one.
    let mut best: Option<((u8, u64), Source)> = None;
    let mut consider = |rank: (u8, u64), source: Source| {
        if best.as_ref().is_none_or(|(kept, _)| rank < *kept) {
            best = Some((rank, source));
        }
    };
    let by_speed = |speed: Option<u64>| speed.map_or((2, 0), |speed| (1, u64::MAX - speed));
    if !tried.hub {
        consider(by_speed(updates.own_speed), Source::Hub);
    }
    for (name, (generation, report)) in candidates {
        let rank = if report.holds.contains(held) {
            (0, 0)
        } else if report.relays && report.github == Some(true) {
            by_speed(report.github_speed)
        } else {
            continue;
        };
        consider(rank, Source::Machine(name.clone(), *generation));
    }
    best.map(|(_, source)| source)
}
