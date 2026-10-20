//! A package a machine asks the hub for: from the hub's own copy; else
//! brought in once, however many ask (`world-network.md`, 中枢替网络存的东西),
//! from where its release published it, by the hub itself or by a machine
//! that holds it or reaches GitHub and fetches for others, the fastest
//! first. Every source is judged the same way (`fastctx_update::pace`):
//! while none going looks like finishing soon, the next is asked beside
//! them, a few at once, and the first to have the whole package, checked,
//! puts it in place. Packages come in one at a time, and every byte under
//! one ceiling (`fastctx_update::throttle`): a source is added only while
//! those going leave the ceiling unused.

use std::collections::VecDeque;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use fastctx_net::wire::{Down, Told};
use fastctx_types::MachineName;
use fastctx_types::update::{Fetch, Held, Manifest, Package};
use fastctx_update::pace::{AT_ONCE, Pace, Verdict};
use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;

use super::{Attempt, Fetching, Race};
use crate::{Hub, Out};

/// How long bringing one package in may take in all, however it is
/// brought, giving its last try up included: within the 45 minutes whoever
/// asks for it waits (`fastctx_net::link`), so that it hears why.
const BUDGET: Duration = Duration::from_secs(43 * 60);

/// How long a try given up has to end its transfer before it is revoked.
const SETTLE: Duration = Duration::from_secs(30);

/// Over how long the bytes coming in are measured against the ceiling.
const MEASURED: Duration = Duration::from_secs(5);

/// The share of the ceiling past which no source is added: those going
/// fill it, and another would only split it with them.
const FILLED: f64 = 0.7;

/// `asker` asks, as its question `id`, for the package `target` of
/// `version`: answered once the hub holds it, or with why it cannot.
#[allow(clippy::too_many_arguments)]
pub(crate) fn ask(
    hub: &Arc<Hub>,
    name: &MachineName,
    generation: u64,
    id: u64,
    version: String,
    target: String,
    reports: bool,
    out: crate::outq::Outq,
) {
    let held = Held { version, target };
    hub.updates
        .lock()
        .expect("never poisoned")
        .requests
        .insert((name.clone(), generation, id), (held.clone(), reports));
    let name = name.clone();
    let hub = hub.clone();
    tokio::spawn(async move {
        let (progress, mut changes) = tokio::sync::watch::channel(Default::default());
        let work = obtain(&hub, held, Some(progress));
        tokio::pin!(work);
        let answer = loop {
            tokio::select! {
                result = &mut work => break result.map(|size| Told::Package { size }),
                _ = out.closed() => return,
                changed = changes.changed(), if reports => {
                    if changed.is_ok() {
                        let progress = *changes.borrow_and_update();
                        if out.send(Out::Frame(Down::PackageProgress { id, progress })).is_err() { return; }
                    }
                }
            }
        };
        if answer.is_err() {
            super::inventory::done(&hub, &name, generation, id);
        }
        let _ = out.send(Out::Frame(Down::Told { id, answer }));
    });
}

/// Brings the package `target` of `version` in with nobody waiting for it:
/// whoever asks next finds it held, or joins the bringing in still going.
pub(crate) fn bring_in(hub: &Arc<Hub>, version: String, target: String) {
    let hub = hub.clone();
    tokio::spawn(async move {
        // Why it could not is in the hub's log already.
        let _ = obtain(&hub, Held { version, target }, None).await;
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
    waiters: Vec<Waiter>,
    expired: bool,
    id: u64,
}

struct Waiter {
    answer: oneshot::Sender<Result<u64, String>>,
    progress: Option<tokio::sync::watch::Sender<fastctx_types::update::PackageProgress>>,
}

/// The package's size once the hub holds it.
pub(super) async fn obtain(
    hub: &Arc<Hub>,
    held: Held,
    progress: Option<tokio::sync::watch::Sender<fastctx_types::update::PackageProgress>>,
) -> Result<u64, String> {
    let cache = hub.updates.lock().expect("never poisoned").cache.clone();
    if let Some(size) = size_of(&cache, &held)? {
        return Ok(size);
    }
    let (answer, answered) = oneshot::channel();
    let waiting = Waiter { answer, progress };
    {
        let mut updates = hub.updates.lock().expect("never poisoned");

        match updates.obtaining.get_mut(&held) {
            Some(owner) if owner.expired => {
                return Err(
                    "the previous package request timed out and its owner is still finishing; try again later".into()
                );
            }
            Some(owner) => {
                owner.waiters.retain(|waiter| !waiter.answer.is_closed());
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
    // One package comes in at a time; its time counts from its turn.
    let coming = hub.updates.lock().expect("never poisoned").coming.clone();
    let _turn = coming.lock_owned().await;
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
        let _ = waiter.answer.send(brought.clone());
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
        let _ = waiter.answer.send(Err(reason.clone()));
    }
}

fn size_of(cache: &fastctx_update::Cache, held: &Held) -> Result<Option<u64>, String> {
    cache.path(&held.version, &held.target)?;
    Ok(cache.get(&held.version, &held.target).and_then(|path| std::fs::metadata(path).ok()).map(|meta| meta.len()))
}

async fn bring(hub: &Arc<Hub>, held: &Held) -> Result<u64, String> {
    let (manifest, cache, intake) = {
        let updates = hub.updates.lock().expect("never poisoned");
        (updates.manifest(&held.version), updates.cache.clone(), updates.intake.clone())
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
    intake.set(hub.store().hub.updates.intake());
    let deadline = Instant::now() + BUDGET;
    let mut measured: VecDeque<(Instant, u64)> = VecDeque::new();
    let race = Arc::new(Race::default());
    let (ended, mut endings) = mpsc::unbounded_channel();
    let mut going: Vec<Going> = Vec::new();
    let mut settling = tokio::task::JoinSet::new();
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
                let progress = try_.attempt.progress(package.size);
                try_.pace.heard(now.into_std(), progress.received);
                try_.pace.verdict(now.into_std(), progress.total)
            })
            .collect();
        if let Some(progress) =
            going.iter().map(|source| source.attempt.progress(package.size)).max_by_key(|progress| progress.received)
        {
            let updates = hub.updates.lock().expect("never poisoned");
            if let Some(owner) = updates.obtaining.get(held) {
                for progress_to in owner.waiters.iter().filter_map(|waiter| waiter.progress.as_ref()) {
                    progress_to.send_if_modified(|kept| {
                        if *kept == progress {
                            false
                        } else {
                            *kept = progress;
                            true
                        }
                    });
                }
            }
        }
        for (index, verdict) in verdicts.iter().enumerate().rev() {
            if *verdict == Verdict::Slow && !going[index].slow {
                going[index].slow = true;
                hub.log.line(format_args!(
                    "bringing {} of {}: {} is slow; trying another source",
                    held.target, held.version, going[index].source
                ));
            }
            if *verdict == Verdict::Stopped {
                let stopped = going.remove(index);
                failures.push(format!("{}: it brought nothing more for half a minute", stopped.source));
                settling.spawn(settle_owned(hub.clone(), cache.clone(), held.clone(), stopped, deadline));
            }
        }
        measured.push_back((now, intake.taken()));
        while measured.front().is_some_and(|(at, _)| now.duration_since(*at) > MEASURED) {
            measured.pop_front();
        }
        let filled = match (measured.front(), measured.back()) {
            (Some((from, before)), Some((to, after))) if to.duration_since(*from) >= MEASURED / 2 => {
                (after - before) as f64 / to.duration_since(*from).as_secs_f64() >= FILLED * intake.rate() as f64
            }
            _ => false,
        };
        let promising = verdicts.contains(&Verdict::Promising);
        if !promising && !filled && going.len() < AT_ONCE {
            match next(hub, held, &tried) {
                Some(Source::Hub) => {
                    tried.hub = true;
                    going.push(by_the_hub(hub, &cache, &manifest, held, &package, &race, &ended, &intake));
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
                        hub.log.line(format_args!("bringing {} of {} from {}: {why}", held.target, held.version, over.source));
                        failures.push(format!("{}: {why}", over.source));
                    }
                    settling.spawn(settle_owned(hub.clone(), cache.clone(), held.clone(), over, deadline));
                }
            }
            _ = look.tick() => {}
        }
    };
    if let Some(size) = size {
        let waiters = hub
            .updates
            .lock()
            .expect("never poisoned")
            .obtaining
            .get_mut(held)
            .map(|owner| std::mem::take(&mut owner.waiters))
            .unwrap_or_default();
        for waiter in waiters {
            let _ = waiter.answer.send(Ok(size));
        }
    }
    // Tell every remaining source to stop before waiting for any cleanup.
    for try_ in &going {
        stop(hub, try_);
    }
    for try_ in going {
        settling.spawn(settle_owned(hub.clone(), cache.clone(), held.clone(), try_, deadline));
    }
    while let Some(ended) = settling.join_next().await {
        if let Err(error) = ended {
            hub.log.line(format_args!("finishing a package source: {error}"));
        }
    }
    if let Some(size) = size {
        return Ok(size);
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
    slow: bool,
}

/// The hub's own download of the package, as one try: its bytes counted as
/// they come, stopped when the try is given up, and put in place only if
/// no other try put the package there first. How fast it went is kept, to
/// weigh the hub against its machines next time.
#[allow(clippy::too_many_arguments)]
fn by_the_hub(
    hub: &Arc<Hub>,
    cache: &fastctx_update::Cache,
    manifest: &Manifest,
    held: &Held,
    package: &Package,
    race: &Arc<Race>,
    ended: &mpsc::UnboundedSender<(u64, Result<(), String>)>,
    intake: &Arc<fastctx_update::throttle::Throttle>,
) -> Going {
    let id = hub.next();
    let attempt = Arc::new(Attempt::new(id, race.clone()));
    attempt.begin();
    let url = package.url.clone().expect("the hub downloads only a package published somewhere");
    let (hub, cache, manifest, held, ended, intake) =
        (hub.clone(), cache.clone(), manifest.clone(), held.clone(), ended.clone(), intake.clone());
    let trying = attempt.clone();
    tokio::spawn(async move {
        let outcome = async {
            let incoming = cache.attempt(&held.version, &held.target, id)?;
            let counting = trying.clone();
            let mut heard = move |brought, total| {
                counting.brought.store(brought, Ordering::Relaxed);
                counting.downloaded.lock().expect("never poisoned").total = total;
            };
            let speed = tokio::select! {
                downloaded = fastctx_update::download::patiently(&url, &incoming, &mut heard, Some(&intake)) => downloaded?,
                () = trying.stopping() => return Err("given up for another".to_string()),
            };
            if let Some(speed) = speed {
                hub.updates.lock().expect("never poisoned").own_speed = Some(speed);
            }
            if !trying.keep() {
                return Err("given up for another".to_string());
            }
            super::transfer::take_in(&hub, &cache, &manifest, &held.target, &incoming, &trying).await.map(|_| ())
        }
        .await;
        trying.over();
        let _ = ended.send((id, outcome));
    });
    Going { source: Source::Hub, attempt, pace: Pace::new(std::time::Instant::now()), slow: false }
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
    let progress = hub
        .updates
        .lock()
        .expect("never poisoned")
        .reports
        .get(&helper)
        .is_some_and(|(_, report)| report.package_progress);
    let fetching = Fetching {
        helper: helper.clone(),
        generation,
        held: held.clone(),
        done: Some(done),
        attempt: attempt.clone(),
        progress,
    };
    hub.updates.lock().expect("never poisoned").fetches.insert(id, fetching);
    let fetch =
        Fetch { id, version: held.version.clone(), target: held.target.clone(), package: package.clone(), progress };
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
        slow: false,
    }
}

fn stop(hub: &Hub, source: &Going) {
    source.attempt.give_up();
    cancel_helper(hub, source.attempt.id);
}

fn cancel_helper(hub: &Hub, id: u64) {
    let helper = hub
        .updates
        .lock()
        .expect("never poisoned")
        .fetches
        .get(&id)
        .filter(|fetching| fetching.progress)
        .map(|fetching| (fetching.helper.clone(), fetching.generation));
    if let Some((name, generation)) = helper
        && let Some(session) = hub.sessions().get(&name).filter(|session| session.generation == generation)
    {
        let _ = session.out.send(Out::Frame(Down::FetchCancelled { id }));
    }
}

pub(super) fn cancel_helpers(hub: &Hub, race: &Arc<Race>, winner: u64) {
    let ids: Vec<u64> = hub
        .updates
        .lock()
        .expect("never poisoned")
        .fetches
        .iter()
        .filter(|(id, fetching)| **id != winner && Arc::ptr_eq(&fetching.attempt.race, race))
        .map(|(id, _)| *id)
        .collect();
    for id in ids {
        cancel_helper(hub, id);
    }
}

/// A helper's received bytes belong to the request on this session only.
pub(crate) fn progress(
    hub: &Hub,
    name: &MachineName,
    generation: u64,
    id: u64,
    progress: fastctx_types::update::PackageProgress,
) {
    let updates = hub.updates.lock().expect("never poisoned");
    if let Some(fetching) =
        updates.fetches.get(&id).filter(|f| f.helper == *name && f.generation == generation && f.progress)
    {
        *fetching.attempt.downloaded.lock().expect("never poisoned") = progress;
    }
}

async fn settle_owned(hub: Arc<Hub>, cache: fastctx_update::Cache, held: Held, source: Going, deadline: Instant) {
    settle(&hub, &cache, &held, source, deadline).await;
}

/// Ends a try: its transfer is told to stop and given a moment to, and
/// what it left is deleted. One with the whole package checks it and puts
/// it in place, if no other did, within what is left of the budget; one
/// that does not end in time can no longer put its package in place, and
/// deletes what it left itself; one putting it in place then finishes.
async fn settle(hub: &Hub, cache: &fastctx_update::Cache, held: &Held, try_: Going, deadline: Instant) {
    stop(hub, &try_);
    let Going { source, attempt, .. } = try_;
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
        if !report.relays {
            continue;
        }
        let rank = if report.holds.contains(held) {
            (0, 0)
        } else if report.github_at.is_some_and(|at| jiff::Timestamp::now().duration_since(at).as_secs() < 24 * 3600)
            || (report.github_at.is_none() && report.github == Some(true))
        {
            by_speed(report.github_speed)
        } else {
            continue;
        };
        consider(rank, Source::Machine(name.clone(), *generation));
    }
    best.map(|(_, source)| source)
}
