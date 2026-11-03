//! Coalesced projection refreshes. A refresh holds its conversation's
//! management gate shared, so what the user does there never waits for it,
//! and the conversation's own refresh lock, so two refreshes, or a refresh
//! and a turn's preparation, never write its blocks at once. Only a few
//! refreshes run in the background at a time; account invalidations carry no
//! context bodies.

use crate::Node;
use fastctx_types::{ResidentId, ipc::Failure};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

/// How long a refresh of every conversation waits for the changes that come
/// with the one that woke it: machines come and go in bursts (a room starting
/// up, a line that keeps dropping), and each would otherwise refresh every
/// conversation here again.
const SETTLE: std::time::Duration = std::time::Duration::from_secs(3);

/// How many conversations' contexts are refreshed in the background at once:
/// the rest wait their turn, holding nothing.
const BACKGROUND: usize = 2;

/// How many times in a row a refresh may fail the same way before it stops
/// being tried on a timer: what it reads did not change, so another try
/// would fail again (a conversation of an account the hub no longer has).
/// A change it reads, or the conversation's next turn, tries it once more.
const SAME_FAILURES: u32 = 4;

pub(crate) struct Updates {
    pending: Mutex<BTreeMap<ResidentId, Pending>>,
    refreshing: crate::leases::Registry<ResidentId, fastctx_platform::doing::Mutex<()>>,
    background: tokio::sync::Semaphore,
}

impl Default for Updates {
    fn default() -> Self {
        Self {
            pending: Mutex::default(),
            refreshing: crate::leases::Registry::default(),
            background: tokio::sync::Semaphore::new(BACKGROUND),
        }
    }
}

impl Updates {
    /// Held through one refresh of `id`'s context.
    pub(crate) fn refreshing(
        &self,
        id: &ResidentId,
    ) -> crate::leases::Lease<ResidentId, fastctx_platform::doing::Mutex<()>> {
        self.refreshing.get_or_insert_with(id, || crate::ranked::refreshing(id))
    }
}

struct Pending {
    revision: u64,
    loaded: u64,
    plan_revision: u64,
    plan_loaded: u64,
    working: bool,
    /// The last failure, and how many times in a row it came.
    failed: Option<(String, u32)>,
}
impl Default for Pending {
    fn default() -> Self {
        Self { revision: 1, loaded: 0, plan_revision: 1, plan_loaded: 0, working: false, failed: None }
    }
}

/// Called while the conversation's management gate is held, shared or alone.
pub(crate) async fn refresh(node: &Arc<Node>, id: &ResidentId) -> Result<(), Failure> {
    let refreshing = node.contexts.refreshing(id);
    let _refreshing = refreshing.lock().await;
    let (revision, plan_revision, read_plan) = {
        let mut pending = node.contexts.pending.lock().expect("never poisoned");
        let pending = pending.entry(id.clone()).or_default();
        if pending.loaded == pending.revision {
            return Ok(());
        }
        (pending.revision, pending.plan_revision, pending.plan_loaded != pending.plan_revision)
    };
    node.residents.active(id)?;
    let (about, town) = node.residents.about(id).ok_or_else(|| Failure::new(super::words::UNKNOWN))?;
    crate::focus::refresh(node, id).await?;
    super::refresh_local(node, id).await.map_err(|error| Failure::new(error.to_string()))?;
    super::sources::plan_guide(node, id, &about.origin, &about.acting)
        .await
        .map_err(|error| Failure::new(error.to_string()))?;
    if read_plan {
        crate::plan::context(node, id, &about.acting).await?;
    }
    super::sources::skills(node, id, &about.acting).await?;
    super::map(node, id, town.as_ref(), &about.origin, &about.acting).await?;
    let mut contexts = node.contexts.pending.lock().expect("never poisoned");
    let pending = contexts.entry(id.clone()).or_default();
    pending.loaded = revision;
    pending.plan_loaded = plan_revision;
    Ok(())
}

fn request(node: &Arc<Node>, id: &ResidentId, plan: bool) {
    {
        let mut pending = node.contexts.pending.lock().expect("never poisoned");
        let pending = pending.entry(id.clone()).or_default();
        pending.revision = pending.revision.wrapping_add(1);
        if plan {
            pending.plan_revision = pending.plan_revision.wrapping_add(1);
        }
        if pending.working {
            return;
        }
        pending.working = true;
    }
    let (node, id) = (node.clone(), id.clone());
    fastctx_platform::task::spawn(async move {
        // Named in what the node reports of a lock held too long.
        let _doing = fastctx_platform::doing::doing(format!("refreshing {id}'s context"));
        let mut again = crate::again::Again::default();
        loop {
            let result = async {
                let _turn = node.contexts.background.acquire().await.expect("never closed");
                let gate = node.residents.management(&id)?;
                let _using = gate.read().await;
                refresh(&node, &id).await
            }
            .await;
            let failure = result.err().map(|error| error.to_string());
            let failed = failure.is_some();
            if node.residents.active(&id).is_ok() {
                told(&node, &id, failure.as_deref());
            }
            {
                let mut pending = node.contexts.pending.lock().expect("never poisoned");
                if node.residents.active(&id).is_err() {
                    pending.remove(&id);
                    break;
                }
                let pending = pending.get_mut(&id).expect("worker owns its entry");
                pending.failed = match (failure, pending.failed.take()) {
                    (None, _) => None,
                    (Some(now), Some((before, times))) if now == before => Some((now, times.saturating_add(1))),
                    (Some(now), _) => Some((now, 1)),
                };
                if !failed && pending.loaded == pending.revision {
                    pending.working = false;
                    break;
                }
                if pending.failed.as_ref().is_some_and(|(_, times)| *times >= SAME_FAILURES) {
                    pending.working = false;
                    break;
                }
            }
            // A refresh that failed is tried again, later each time, and a
            // change meanwhile is taken in with it; one that a change
            // outdated while it ran runs again at once.
            if failed {
                tokio::time::sleep(again.next()).await;
            } else {
                again.reset();
            }
        }
    });
}

/// Logs how a refresh of `id`'s context failed, when it starts failing or
/// fails otherwise than it did; a success clears it.
pub(crate) fn told(node: &Node, id: &ResidentId, failure: Option<&str>) {
    if node.said.changed(id, "context", failure) {
        node.log.line(format_args!("refreshing {id}'s context: {}", failure.unwrap_or_default()));
    }
}

/// Refreshes `id`'s context with its plan read again: its plan or working
/// place changed.
pub(crate) fn request_plan(node: &Arc<Node>, id: &ResidentId) {
    request(node, id, true);
}

fn request_each(node: &Arc<Node>, plan: bool) {
    for resident in node.residents.everyone().filter(|resident| resident.removed.is_none()) {
        request(node, &resident.id, plan);
    }
}

pub(crate) fn start(node: &Arc<Node>) {
    super::records::start(node);
    let node = node.clone();
    fastctx_platform::task::spawn(async move {
        let mut views = node.views.subscribe();
        let mut ways = node.ways.subscribe();
        let mut memory = node.memory_changes.subscribe();
        let mut plan = true;
        let mut seen = presence(&node);
        loop {
            let working = node.clone();
            if let Err(error) = fastctx_platform::task::spawn_blocking(move || request_each(&working, plan)).await {
                node.log.line(format_args!("refreshing conversation contexts: {error}"));
            }
            plan = loop {
                // Connectivity changes the authorized focus and memory paths,
                // but never the plan. Plan records and reconnects invalidate
                // it separately; a refresh that fails is tried again on its own.
                let (viewed, remembered) = tokio::select! {
                    _ = views.changed() => (true, false),
                    _ = ways.changed() => (false, false),
                    _ = memory.changed() => (false, true),
                };
                tokio::time::sleep(SETTLE).await;
                // What changed meanwhile is in this refresh, and is not seen again.
                let viewed = viewed || views.has_changed().unwrap_or(false);
                let remembered = remembered || memory.has_changed().unwrap_or(false);
                views.mark_unchanged();
                ways.mark_unchanged();
                memory.mark_unchanged();
                // A phone coming and going, a path or a strain, moves nothing a
                // context reads: the contexts are left as they are.
                let now = presence(&node);
                let moved = now != seen;
                seen = now;
                if viewed || remembered || moved {
                    break viewed;
                }
            };
        }
    });
}

/// What a context reads of the hubs' presence, as one number: whether each
/// line is up with a current view, and which registered computers are
/// online, since when.
fn presence(node: &Node) -> u64 {
    use std::hash::{Hash as _, Hasher as _};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for link in node.links() {
        link.membership().id.to_string().hash(&mut hasher);
        (matches!(link.status(), fastctx_net::link::Status::Online { .. }) && link.view_current()).hash(&mut hasher);
        let view = link.view();
        let mut machines: Vec<_> = view.machines().map(|member| (&member.name, member.online, member.since)).collect();
        machines.sort_unstable();
        machines.hash(&mut hasher);
    }
    hasher.finish()
}
