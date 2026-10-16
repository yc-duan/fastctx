//! A package a machine asks the hub for: from the hub's own copy; else
//! brought in once, however many ask, from where its release published it,
//! and when the hub cannot reach that, from the machines that hold it or can
//! reach GitHub and fetch for others, one after another.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use fastctx_net::wire::{Down, Told};
use fastctx_types::MachineName;
use fastctx_types::update::{Fetch, Held};
use tokio::sync::oneshot;
use tokio::time::Instant;

use super::{Attempt, Fetching};
use crate::{Hub, Out};

/// How long a machine asked to fetch a package has to deliver it.
const HELPER_PATIENCE: Duration = Duration::from_secs(30 * 60);

/// How long a machine asked to fetch a package may bring nothing more of
/// it before the next is asked.
const STALLED: Duration = Duration::from_secs(5 * 60);

/// How long the hub may download a package from where its release is
/// published before it asks the machines instead.
const DOWNLOAD_PATIENCE: Duration = Duration::from_secs(20 * 60);

/// How long bringing one package in may take in all, however it is
/// brought, giving its last try up included: within the 45 minutes whoever
/// asks for it waits (`fastctx_net::link`), so that it hears why.
const BUDGET: Duration = Duration::from_secs(43 * 60);

/// How long a try given up has to end its transfer before the next starts.
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
    let manifest = manifest.ok_or_else(|| format!("no machine of the network knows the release {}", held.version))?;
    let package = manifest
        .package(&held.target)
        .cloned()
        .ok_or_else(|| format!("the release {} has no package for {}", held.version, held.target))?;
    let mut failures = Vec::new();
    let deadline = Instant::now() + BUDGET;
    if let Some(url) = &package.url {
        let incoming = cache.attempt(&held.version, &held.target, hub.next())?;
        let patience = DOWNLOAD_PATIENCE.min(deadline.saturating_duration_since(Instant::now()));
        let mut quiet = |_, _| {};
        let downloaded = tokio::time::timeout(patience, fastctx_update::download::from_url(url, &incoming, &mut quiet));
        let result = match downloaded.await {
            Ok(Ok(())) => super::transfer::take_in(hub, &cache, &manifest, &held.target, &incoming, || true).await,
            Ok(Err(why)) => Err(why),
            Err(_) => Err(format!("downloading it took more than {} minutes", patience.as_secs() / 60)),
        };
        super::cleanup::temporary(hub, &incoming).await;
        match result {
            Ok(size) => return Ok(size),
            Err(why) => failures.push(why),
        }
    }
    let mut tried = Vec::new();
    while let Some((helper, generation)) = choose(hub, held, &tried) {
        if Instant::now() >= deadline {
            failures.push(format!("no machine brought it within {} minutes in all", BUDGET.as_secs() / 60));
            break;
        }
        tried.push(helper.clone());
        let id = hub.next();
        let attempt = Arc::new(Attempt::new(id));
        let (done, delivered) = oneshot::channel();
        let fetching = Fetching {
            helper: helper.clone(),
            generation,
            held: held.clone(),
            done: Some(done),
            attempt: attempt.clone(),
        };
        hub.updates.lock().expect("never poisoned").fetches.insert(id, fetching);
        let _asking = Asking { hub, id, attempt: attempt.clone() };
        let fetch = Fetch { id, version: held.version.clone(), target: held.target.clone(), package: package.clone() };
        let sent = hub
            .sessions()
            .get(&helper)
            .filter(|session| session.generation == generation)
            .is_some_and(|session| session.out.send(Out::Frame(Down::Fetch { fetch: Box::new(fetch) })).is_ok());
        let outcome = if sent {
            hub.log.line(format_args!("asked {helper} to fetch {} of {}", held.target, held.version));
            watch_attempt(&attempt, delivered, deadline).await
        } else {
            Err("it left before it was asked".to_string())
        };
        // The try is over before the next begins: its transfer is told to
        // stop and given a moment to, and what it left is deleted. One with
        // the whole package checks it and puts it in place within what is
        // left of the budget. One that does not end in time can no longer
        // put its package in place, and deletes what it left itself; one
        // putting it in place then finishes.
        attempt.give_up();
        let settle = match attempt.keeping() {
            true => deadline.max(Instant::now() + SETTLE),
            false => Instant::now() + SETTLE,
        };
        if tokio::time::timeout_at(settle, attempt.ended()).await.is_err() {
            hub.log.line(format_args!(
                "the transfer from {helper} did not end in time; it may no longer put its package in place"
            ));
            attempt.revoke();
            attempt.ended().await;
        }
        hub.updates.lock().expect("never poisoned").fetches.remove(&id);
        if attempt.is_over()
            && let Ok(left) = cache.attempt(&held.version, &held.target, id)
        {
            super::cleanup::temporary(hub, &left).await;
        }
        // Whatever its machine said, what it brought may be in place now.
        if let Some(size) = size_of(&cache, held)? {
            return Ok(size);
        }
        match outcome {
            Ok(()) => failures.push(format!("{helper} delivered, but the package is not here")),
            Err(why) => failures.push(format!("{helper}: {why}")),
        }
    }
    if failures.is_empty() {
        return Err("no machine of the network can reach GitHub or holds the package now".to_string());
    }
    Err(failures.join("; "))
}

/// What came of the try a machine was asked to make: what it said, or why
/// it was given up: it took longer than its patience or than what is left
/// of `deadline`, or its transfer brought nothing more for `STALLED`. Before
/// its transfer starts the machine may be fetching the package itself,
/// which the hub does not see, so only its patience counts then.
async fn watch_attempt(
    attempt: &Attempt,
    mut delivered: oneshot::Receiver<Result<(), String>>,
    deadline: Instant,
) -> Result<(), String> {
    let started = Instant::now();
    let patience = (started + HELPER_PATIENCE).min(deadline);
    let mut seen: Option<u64> = None;
    let mut moved = started;
    loop {
        // Once it has the whole package, only the budget counts while the
        // hub checks it.
        let until = if attempt.keeping() { deadline } else { patience };
        let look = (Instant::now() + Duration::from_secs(15)).min(until);
        tokio::select! {
            outcome = &mut delivered => return outcome.unwrap_or_else(|_| Err("the request was dropped".to_string())),
            () = tokio::time::sleep_until(look) => {}
        }
        let now = Instant::now();
        if attempt.keeping() {
            if now >= deadline {
                return Err("checking what it brought took the rest of the budget".to_string());
            }
            continue;
        }
        if attempt.moving() || seen.is_some() {
            let brought = attempt.brought.load(Ordering::Relaxed);
            if seen != Some(brought) {
                seen = Some(brought);
                moved = now;
            }
            if now.duration_since(moved) >= STALLED {
                return Err(format!("its transfer brought nothing more for {} minutes", STALLED.as_secs() / 60));
            }
        }
        if now >= patience {
            return Err(format!(
                "it did not deliver within {} minutes",
                patience.duration_since(started).as_secs() / 60
            ));
        }
    }
}

/// A try asked of a machine, given up however bringing the package in ends,
/// as when it runs out of time: its transfer stops, a package it checks is
/// not put in place, and its request is forgotten.
struct Asking<'a> {
    hub: &'a Hub,
    id: u64,
    attempt: Arc<Attempt>,
}

impl Drop for Asking<'_> {
    fn drop(&mut self) {
        self.attempt.give_up();
        self.attempt.revoke();
        self.hub.updates.lock().expect("never poisoned").fetches.remove(&self.id);
    }
}

/// The next machine to ask, not one in `tried`: one that holds the package
/// first, then one that reached GitHub and fetches for others.
fn choose(hub: &Hub, held: &Held, tried: &[MachineName]) -> Option<(MachineName, u64)> {
    let sessions = hub.sessions();
    let updates = hub.updates.lock().expect("never poisoned");
    let online = |name: &MachineName, generation: u64| {
        sessions.get(name).is_some_and(|session| session.versions && session.generation == generation)
    };
    let candidates =
        updates.reports.iter().filter(|(name, (generation, _))| !tried.contains(name) && online(name, *generation));
    let mut best: Option<(u8, MachineName, u64)> = None;
    for (name, (generation, report)) in candidates {
        let rank = if report.holds.contains(held) {
            0
        } else if report.relays && report.github == Some(true) {
            1
        } else {
            continue;
        };
        if best.as_ref().is_none_or(|(kept, _, _)| rank < *kept) {
            best = Some((rank, name.clone(), *generation));
        }
    }
    best.map(|(_, name, generation)| (name, generation))
}
