//! Coalesced refreshes of changed sources. Each conversation keeps its own
//! refresh lock and shared management gate; at most two background refreshes run.

use super::{
    dependencies,
    parts::{self, COUNT, Part, Parts},
};
use crate::Node;
use fastctx_types::{ResidentId, ipc::Failure};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
};

const SETTLE: std::time::Duration = std::time::Duration::from_secs(3);
const BACKGROUND: usize = 2;
/// An unchanged failure stops after four attempts, until its inputs change.
const SAME_FAILURES: u32 = 4;

pub(crate) struct Updates {
    pending: Mutex<BTreeMap<ResidentId, Pending>>,
    wake: tokio::sync::Notify,
    pub(crate) routes: super::routes::Routes,
    pub(crate) projects: super::projects::Projects,
    pub(crate) history_catalogs: crate::history::context::catalog::Catalogs,
    refreshing: crate::leases::Registry<ResidentId, fastctx_platform::doing::Mutex<()>>,
    background: tokio::sync::Semaphore,
}

impl Default for Updates {
    fn default() -> Self {
        Self {
            pending: Mutex::default(),
            routes: super::routes::Routes::default(),
            projects: super::projects::Projects::default(),
            history_catalogs: crate::history::context::catalog::Catalogs::default(),
            wake: tokio::sync::Notify::new(),
            refreshing: crate::leases::Registry::default(),
            background: tokio::sync::Semaphore::new(BACKGROUND),
        }
    }
}

impl Updates {
    pub(crate) fn refreshing(
        &self,
        id: &ResidentId,
    ) -> crate::leases::Lease<ResidentId, fastctx_platform::doing::Mutex<()>> {
        self.refreshing.get_or_insert_with(id, || crate::ranked::refreshing(id))
    }

    pub(super) fn request(&self, id: &ResidentId, parts: Parts) {
        if parts.empty() {
            return;
        }
        self.pending.lock().expect("never poisoned").entry(id.clone()).or_default().invalidate(parts);
        self.wake.notify_one();
    }
}

struct Pending {
    inputs: Option<[u64; COUNT]>,
    observed: Option<(u64, u64, u64)>,
    revision: [u64; COUNT],
    loaded: [u64; COUNT],
    working: bool,
    stopped: bool,
    failed: Option<(String, u32)>,
}

impl Default for Pending {
    fn default() -> Self {
        Self {
            inputs: None,
            observed: None,
            revision: [1; COUNT],
            loaded: [0; COUNT],
            working: false,
            stopped: false,
            failed: None,
        }
    }
}

impl Pending {
    fn current(&self, observed: (u64, u64, u64)) -> bool {
        self.revision == self.loaded && self.observed == Some(observed)
    }

    fn observe(&mut self, now: [u64; COUNT], observed: (u64, u64, u64)) {
        if let Some(before) = self.inputs {
            let changed = Parts::changed(&before, &now);
            if !changed.empty() {
                self.invalidate(changed);
            }
        }
        self.inputs = Some(now);
        self.observed = Some(observed);
    }

    fn invalidate(&mut self, changed: Parts) {
        for part in parts::ALL {
            if changed.contains(part) {
                self.revision[part as usize] = self.revision[part as usize].wrapping_add(1);
            }
        }
        self.stopped = false;
    }
    fn completed(&mut self, revision: [u64; COUNT], failure: Option<String>) -> bool {
        let failed = failure.is_some();
        if self.revision == revision {
            self.failed = match (failure, self.failed.take()) {
                (None, _) => None,
                (Some(now), Some((before, times))) if now == before => Some((now, times.saturating_add(1))),
                (Some(now), _) => Some((now, 1)),
            };
            self.stopped = self.failed.as_ref().is_some_and(|(_, times)| *times >= SAME_FAILURES);
        }
        if self.stopped || (!failed && self.loaded == self.revision) {
            self.working = false;
            return true;
        }
        false
    }
}

// Directory counters avoid repeated scans; each part still decides whether its inputs changed.
fn observation(
    node: &Node,
    about: &fastctx_types::residents::About,
    town: Option<&fastctx_types::TownName>,
) -> (u64, u64, u64) {
    (*node.views.borrow(), *node.ways.borrow(), dependencies::hash(&(about, town)))
}

/// Called while the conversation's management gate is held, shared or alone.
pub(crate) async fn refresh(node: &Arc<Node>, id: &ResidentId) -> Result<(), Failure> {
    let refreshing = node.contexts.refreshing(id);
    let _refreshing = refreshing.lock().await;
    node.residents.active(id)?;
    let (about, town) = node.residents.about(id).ok_or_else(|| Failure::new(super::words::UNKNOWN))?;
    let observed = observation(node, &about, town.as_ref());
    if node.contexts.pending.lock().expect("never poisoned").get(id).is_some_and(|pending| pending.current(observed)) {
        return Ok(());
    }
    let inputs = dependencies::Scan::default().inputs(node, &about, town.as_ref());
    let (revision, loaded) = {
        let mut pending = node.contexts.pending.lock().expect("never poisoned");
        let pending = pending.entry(id.clone()).or_default();
        pending.observe(inputs, observed);
        (pending.revision, pending.loaded)
    };
    if revision == loaded {
        return Ok(());
    }
    for part in parts::ALL {
        let index = part as usize;
        if revision[index] == loaded[index] {
            continue;
        }
        let generation = (*node.views.borrow(), *node.ways.borrow(), *node.memory_changes.borrow());
        part.record();
        match part {
            Part::Focus => {
                crate::focus::refresh(node, id).await?;
                super::blocking::with(node, id, |node, id| super::sources::refresh_part_now(node, id, Part::Focus))
                    .await
                    .map_err(crate::record_migration::failure)?;
            }
            Part::User | Part::Project => {
                super::blocking::with(node, id, move |node, id| super::sources::refresh_part_now(node, id, part))
                    .await
                    .map_err(crate::record_migration::failure)?;
            }
            Part::Guide => super::sources::plan_guide(node, id, &about.origin, &about.acting)
                .await
                .map_err(crate::record_migration::failure)?,
            Part::Plan => crate::plan::context(node, id, &about.acting).await?,
            Part::Skills => super::sources::skills(node, id, &about.acting).await?,
            Part::History => super::map(node, id, town.as_ref(), &about.origin, &about.acting).await?,
        }
        if generation != (*node.views.borrow(), *node.ways.borrow(), *node.memory_changes.borrow()) {
            continue;
        }
        // A later source may fail; the successful ones need not run again.
        // A change arriving during this source retains its newer revision.
        if let Some(pending) = node.contexts.pending.lock().expect("never poisoned").get_mut(id) {
            pending.loaded[index] = revision[index];
        }
    }
    Ok(())
}

fn start_pending(node: &Arc<Node>, id: &ResidentId) {
    if !node.residents.host_open(id) {
        return;
    }
    {
        let mut entries = node.contexts.pending.lock().expect("never poisoned");
        let Some(pending) = entries.get_mut(id) else {
            return;
        };
        if pending.working || pending.stopped || pending.revision == pending.loaded {
            return;
        }
        pending.working = true;
    }
    let (node, id) = (node.clone(), id.clone());
    fastctx_platform::task::spawn(async move {
        let _doing = fastctx_platform::doing::doing(format!("refreshing {id}'s context"));
        let mut again = crate::again::Again::default();
        loop {
            let revision =
                node.contexts.pending.lock().expect("never poisoned").get(&id).expect("worker owns entry").revision;
            let result = async {
                let _turn = node.contexts.background.acquire().await.expect("never closed");
                let gate = node.residents.management(&id)?;
                let _using = gate.read().await;
                if !node.residents.host_open(&id) {
                    return Ok(());
                }
                refresh(&node, &id).await
            }
            .await;
            let failure = result.err().map(|error| error.to_string());
            let failed = failure.is_some();
            let active = node.residents.active(&id).is_ok();
            if active {
                told(&node, &id, failure.as_deref());
            }
            {
                let mut entries = node.contexts.pending.lock().expect("never poisoned");
                if !active {
                    entries.remove(&id);
                    break;
                }
                let pending = entries.get_mut(&id).expect("worker owns its entry");
                if !node.residents.host_open(&id) {
                    pending.working = false;
                    break;
                }
                if pending.completed(revision, failure) {
                    break;
                }
            }
            // A refresh that failed is tried again, later each time. One that
            // a change outdated while it ran waits for changes to settle, so a
            // stream of them, as a replica's sync sends, runs it a few times
            // rather than back to back.
            if failed {
                tokio::time::sleep(again.next()).await;
            } else {
                again.reset();
                tokio::time::sleep(SETTLE).await;
            }
        }
    });
}

pub(crate) fn told(node: &Node, id: &ResidentId, failure: Option<&str>) {
    if node.said.changed(id, "context", failure) {
        node.log.line(format_args!("refreshing {id}'s context: {}", failure.unwrap_or_default()));
    }
}

fn scan(node: &Arc<Node>) {
    let generation = (*node.views.borrow(), *node.ways.borrow());
    let mut sources = dependencies::Scan::default();
    let mut live = BTreeSet::new();
    let mut projects = BTreeSet::new();
    for resident in node.residents.everyone().filter(|resident| resident.removed.is_none()) {
        live.insert(resident.id.clone());
        if let Some(town) = &resident.town {
            projects.insert((serde_json::to_string(&resident.about.acting).expect("acting serializes"), town.clone()));
        }
        if !node.residents.host_open(&resident.id) {
            continue;
        }
        let observed = (generation.0, generation.1, dependencies::hash(&(&resident.about, resident.town.as_ref())));
        if node
            .contexts
            .pending
            .lock()
            .expect("never poisoned")
            .get(&resident.id)
            .is_some_and(|pending| pending.current(observed))
        {
            continue;
        }
        let now = sources.inputs(node, &resident.about, resident.town.as_ref());
        node.contexts
            .pending
            .lock()
            .expect("never poisoned")
            .entry(resident.id.clone())
            .or_default()
            .observe(now, observed);
        start_pending(node, &resident.id);
    }

    node.contexts.routes.retain(&live);
    node.contexts.projects.retain(&projects);
    node.contexts.pending.lock().expect("never poisoned").retain(|id, pending| pending.working || live.contains(id));
}

pub(crate) fn start(node: &Arc<Node>) {
    super::records::start(node);
    let node = node.clone();
    fastctx_platform::task::spawn(async move {
        let mut views = node.views.subscribe();
        let mut ways = node.ways.subscribe();
        let mut residents = node.residents.changes();
        loop {
            let working = node.clone();
            if let Err(error) = fastctx_platform::task::spawn_blocking(move || scan(&working)).await {
                node.log.line(format_args!("refreshing conversation contexts: {error}"));
            }
            tokio::select! {
                _ = views.changed() => {},
                _ = ways.changed() => {},
                _ = residents.changed() => {},
                () = node.contexts.wake.notified() => {},
            }
            tokio::time::sleep(SETTLE).await;
            // Changes during the next scan stay pending for the following scan.
            views.mark_unchanged();
            ways.mark_unchanged();
            residents.mark_unchanged();
        }
    });
}
