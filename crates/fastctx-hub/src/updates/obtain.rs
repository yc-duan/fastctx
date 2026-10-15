//! A package a machine asks the hub for: from the hub's own copy; else
//! brought in once, however many ask, from where its release published it,
//! and when the hub cannot reach that, from the machines that hold it or can
//! reach GitHub and fetch for others, one after another.

use std::sync::Arc;
use std::time::Duration;

use fastctx_net::wire::{Down, Told};
use fastctx_types::MachineName;
use fastctx_types::update::{Fetch, Held};
use tokio::sync::{mpsc, oneshot};

use super::Fetching;
use crate::{Hub, Out};

/// How long a machine asked to fetch a package has to deliver it.
const HELPER_PATIENCE: Duration = Duration::from_secs(30 * 60);

/// `asker` asks, as its question `id`, for the package `target` of
/// `version`: answered once the hub holds it, or with why it cannot.
pub(crate) fn ask(hub: &Arc<Hub>, id: u64, version: String, target: String, out: mpsc::UnboundedSender<Out>) {
    let hub = hub.clone();
    tokio::spawn(async move {
        let answer = obtain(&hub, Held { version, target }).await.map(|size| Told::Package { size });
        let _ = out.send(Out::Frame(Down::Told { id, answer }));
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

/// The package's size once the hub holds it.
async fn obtain(hub: &Arc<Hub>, held: Held) -> Result<u64, String> {
    let (waiting, answered) = oneshot::channel();
    {
        let mut updates = hub.updates.lock().expect("never poisoned");
        if let Some(size) = size_of(&updates.cache, &held)? {
            return Ok(size);
        }
        match updates.obtaining.get_mut(&held) {
            Some(waiters) => waiters.push(waiting),
            None => {
                updates.obtaining.insert(held.clone(), vec![waiting]);
                let (hub, held) = (hub.clone(), held.clone());
                tokio::spawn(async move {
                    let brought = bring(&hub, &held).await;
                    if let Err(why) = &brought {
                        hub.log.line(format_args!("bringing in {} of {}: {why}", held.target, held.version));
                    }
                    let waiters = hub.updates.lock().expect("never poisoned").obtaining.remove(&held);
                    for waiter in waiters.into_iter().flatten() {
                        let _ = waiter.send(brought.clone());
                    }
                });
            }
        }
    }
    answered.await.map_err(|_| "the hub stopped bringing it in".to_string())?
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
    let manifest = manifest.ok_or_else(|| format!("no machine of the network knows the release {}", held.version))?;
    let package = manifest
        .package(&held.target)
        .cloned()
        .ok_or_else(|| format!("the release {} has no package for {}", held.version, held.target))?;
    let mut failures = Vec::new();
    if let Some(url) = &package.url {
        let incoming = cache.incoming(&held.version, &held.target)?;
        let _ = std::fs::remove_file(&incoming);
        match fastctx_update::download::from_url(url, &incoming, &mut |_, _| {}).await {
            Ok(()) => match super::transfer::take_in(hub, &cache, &manifest, &held.target, &incoming).await {
                Ok(size) => return Ok(size),
                Err(why) => failures.push(why),
            },
            Err(why) => failures.push(why),
        }
    }
    let mut tried = Vec::new();
    while let Some((helper, generation)) = choose(hub, held, &tried) {
        tried.push(helper.clone());
        let id = hub.next();
        let (done, delivered) = oneshot::channel();
        hub.updates
            .lock()
            .expect("never poisoned")
            .fetches
            .insert(id, Fetching { helper: helper.clone(), generation, held: held.clone(), done: Some(done) });
        let fetch = Fetch { id, version: held.version.clone(), target: held.target.clone(), package: package.clone() };
        let sent = hub
            .sessions()
            .get(&helper)
            .filter(|session| session.generation == generation)
            .is_some_and(|session| session.out.send(Out::Frame(Down::Fetch { fetch: Box::new(fetch) })).is_ok());
        let outcome = if sent {
            hub.log.line(format_args!("asked {helper} to fetch {} of {}", held.target, held.version));
            match tokio::time::timeout(HELPER_PATIENCE, delivered).await {
                Ok(Ok(outcome)) => outcome,
                Ok(Err(_)) => Err("the request was dropped".to_string()),
                Err(_) => Err(format!("it did not deliver within {} minutes", HELPER_PATIENCE.as_secs() / 60)),
            }
        } else {
            Err("it left before it was asked".to_string())
        };
        hub.updates.lock().expect("never poisoned").fetches.remove(&id);
        match outcome {
            Ok(()) => {
                if let Some(size) = size_of(&cache, held)? {
                    return Ok(size);
                }
                failures.push(format!("{helper} delivered, but the package is not here"));
            }
            Err(why) => failures.push(format!("{helper}: {why}")),
        }
    }
    if failures.is_empty() {
        return Err("no machine of the network can reach GitHub or holds the package now".to_string());
    }
    Err(failures.join("; "))
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
