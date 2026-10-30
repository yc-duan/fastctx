//! Compatibility for local plan files on older hubs, and retirement of
//! files left on assisting hosts. New account copies use placement::copies.
//! File notifications and directory changes replace conversation polling.

mod legacy;

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use fastctx_types::ResidentId;
use fastctx_types::plan::Page;
use fastctx_types::residents::Origin;

use crate::Node;

/// How long a file the hub did not take waits before it is offered again.
const RETRY: Duration = Duration::from_secs(30);

/// What the node last saw of a plan file, and what of it the hub holds.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Synced {
    /// The file's time and size when last looked at.
    stamp: Option<(SystemTime, u64)>,
    /// The text the hub last took from the file or wrote into it; none when
    /// the file may hold text the hub never took.
    text: Option<u64>,
    /// When to offer the file again, after the hub did not take it.
    retry: Option<Instant>,
}

#[derive(Default)]
pub(crate) struct PlanFiles(
    Mutex<HashMap<ResidentId, Synced>>,
    Mutex<HashMap<ResidentId, bool>>,
    Mutex<Option<fastctx_watch::Watcher>>,
);

/// Where the plan of the conversation `id` is kept.
pub(crate) fn path(node: &Node, id: &ResidentId) -> PathBuf {
    node.world.conversation(id).path().join("plan.md")
}

fn hash(text: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

fn stamp(path: &std::path::Path) -> Option<(SystemTime, u64)> {
    std::fs::metadata(path).ok().map(|meta| (meta.modified().unwrap_or(SystemTime::UNIX_EPOCH), meta.len()))
}

pub(crate) fn start(node: &Arc<Node>) {
    let weak = Arc::downgrade(node);
    let runtime = tokio::runtime::Handle::current();
    let mut watcher = fastctx_watch::Watcher::new(move |event: fastctx_watch::Result<fastctx_watch::Event>| {
        let _entered = runtime.enter();
        let Some(node) = weak.upgrade() else { return };
        match event {
            Ok(event) => {
                for path in event.paths {
                    if path.file_name().is_some_and(|name| name == "plan.md")
                        && let Some(id) =
                            path.parent().and_then(|parent| parent.file_name()).and_then(|name| name.to_str())
                        && let Ok(id) = ResidentId::try_from(id)
                    {
                        request(&node, &id);
                    }
                }
            }
            Err(error) => node.log.line(format_args!("watching legacy plans: {error}")),
        }
    })
    .following_links(false);
    if let Err(error) = watcher.watch_within(&node.world.conversations(), |path| path.components().count() <= 1) {
        node.log.line(format_args!("watching legacy plan directories: {error}"));
    }
    *node.plan_files.2.lock().expect("never poisoned") = Some(watcher);
    let weak = Arc::downgrade(node);
    let mut views = node.views.subscribe();
    tokio::spawn(async move {
        loop {
            let Some(node) = weak.upgrade() else { return };
            for resident in node.residents.everyone().filter(|resident| resident.here && resident.removed.is_none()) {
                request(&node, &resident.id);
            }
            drop(node);
            if views.changed().await.is_err() {
                return;
            }
        }
    });
}

pub(crate) fn request(node: &Arc<Node>, id: &ResidentId) {
    if node.plan_files.1.lock().expect("never poisoned").insert(id.clone(), true).is_some() {
        return;
    }
    let (weak, id) = (Arc::downgrade(node), id.clone());
    tokio::spawn(async move {
        loop {
            let Some(node) = weak.upgrade() else { return };
            node.plan_files.1.lock().expect("never poisoned").insert(id.clone(), false);
            look(&node, &id).await;
            let retry = node.plan_files.0.lock().expect("never poisoned").get(&id).and_then(|state| state.retry);
            if let Some(at) = retry {
                drop(node);
                tokio::time::sleep_until(tokio::time::Instant::from_std(at)).await;
                continue;
            }
            let changed = node
                .plan_files
                .0
                .lock()
                .expect("never poisoned")
                .get(&id)
                .is_some_and(|state| state.stamp != stamp(&path(&node, &id)));
            let mut workers = node.plan_files.1.lock().expect("never poisoned");
            if changed || workers.get(&id) == Some(&true) {
                continue;
            }
            workers.remove(&id);
            return;
        }
    });
}

/// Takes each plan file changed since it was last looked at.
async fn look(node: &Arc<Node>, wanted: &ResidentId) {
    let held: Vec<ResidentId> = node
        .residents
        .everyone()
        .filter(|held| {
            held.id == *wanted
                && held.here
                && held.removed.is_none()
                && held.about.world_managed()
                && matches!(held.about.origin, Origin::User)
        })
        .map(|held| held.id)
        .collect();
    for id in held {
        if allowed(node, &id) == Ok(true)
            && let Some((about, _)) = node.residents.about(&id)
            && let Ok(Some(account)) = crate::plan::file_account(node, &id, &about.acting)
            && node.link_of(&account.hub).is_some_and(|link| link.view().plan_copies)
        {
            node.plan_files.0.lock().expect("never poisoned").remove(&id);
            continue;
        }
        let path = path(node, &id);
        let now = stamp(&path);
        let before = node.plan_files.0.lock().expect("never poisoned").get(&id).copied();
        match allowed(node, &id) {
            Ok(true) => {}
            result => {
                if before.and_then(|before| before.retry).is_some_and(|at| Instant::now() < at) {
                    continue;
                }
                let result = match result {
                    Ok(false) => legacy::retire(node, &id).await,
                    Err(failure) => Err(failure),
                    Ok(true) => unreachable!(),
                };
                let retry = result.is_err().then(|| Instant::now() + RETRY);
                if let Err(failure) = result
                    && before.is_none()
                {
                    node.log.line(format_args!("retiring the legacy plan of {id}: {}", failure.message));
                }
                node.plan_files
                    .0
                    .lock()
                    .expect("never poisoned")
                    .insert(id.clone(), Synced { stamp: now, text: None, retry });
                continue;
            }
        }
        // First looked at: the plan as the hub keeps it comes into the file
        // (`pulled`). Where the hub could not be read, a file left from
        // before is the plan's latest word and is offered on the next look;
        // with no hub to keep it, the file is taken as it is.
        let Some(before) = before else {
            if let Some((about, _)) = node.residents.about(&id) {
                let _ = crate::plan::context(node, &id, &about.origin, &about.acting).await;
            }
            let text = now.is_none().then(|| hash(""));
            node.plan_files.0.lock().expect("never poisoned").entry(id.clone()).or_insert(Synced {
                stamp: None,
                text,
                retry: None,
            });
            continue;
        };
        let due = before.retry.is_some_and(|at| Instant::now() >= at);
        if before.stamp == now && !due {
            continue;
        }
        let text = match &now {
            Some(_) => match std::fs::read(&path) {
                Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
                // Unreadable for now (held open, permissions): looked at
                // again in a while rather than at once.
                Err(_) => {
                    let waiting = Synced { stamp: before.stamp, text: before.text, retry: Some(Instant::now() + RETRY) };
                    node.plan_files.0.lock().expect("never poisoned").insert(id.clone(), waiting);
                    continue;
                }
            },
            None => String::new(),
        };
        let seen = hash(&text);
        if Some(seen) == before.text {
            let taken = Synced { stamp: now, text: before.text, retry: None };
            node.plan_files.0.lock().expect("never poisoned").insert(id.clone(), taken);
            continue;
        }
        match crate::plan::save(node, &id, text).await {
            Ok(_) => {
                let taken = Synced { stamp: now, text: Some(seen), retry: None };
                node.plan_files.0.lock().expect("never poisoned").insert(id.clone(), taken);
            }
            Err(failure) => {
                // Still the hub's to take: offered again in a while, whether
                // or not the file changes meanwhile.
                let waiting = Synced { stamp: now, text: before.text, retry: Some(Instant::now() + RETRY) };
                node.plan_files.0.lock().expect("never poisoned").insert(id.clone(), waiting);
                if before.retry.is_none() {
                    node.log.line(format_args!("taking the plan of {id} from its file: {}", failure.message));
                }
            }
        }
    }
}

/// The plan as the hub keeps it, read for the conversation `id`: written
/// into its file where the file holds nothing not taken yet, and noted as
/// what the file was last seen to hold.
pub(crate) fn pulled(node: &Node, id: &ResidentId, page: &Page) {
    let Some((about, _)) = node.residents.about(id) else { return };
    if !about.world_managed() || !matches!(about.origin, Origin::User) || allowed(node, id) != Ok(true) {
        return;
    }
    if let Ok(Some(account)) = crate::plan::file_account(node, id, &about.acting)
        && node.link_of(&account.hub).is_some_and(|link| link.view().plan_copies)
    {
        return;
    }
    let path = path(node, id);
    let markdown = page.body.as_ref().map(|body| body.markdown.as_str()).unwrap_or_default();
    let mut files = node.plan_files.0.lock().expect("never poisoned");
    let on_disk = std::fs::read(&path).ok().map(|bytes| String::from_utf8_lossy(&bytes).into_owned());
    let pending = match (files.get(id), &on_disk) {
        // Changed since last taken, or never taken: the file wins and is
        // taken next.
        (Some(synced), Some(text)) => Some(hash(text)) != synced.text,
        (Some(synced), None) => synced.text != Some(hash("")),
        // First seen: a file left from before is the plan's latest word.
        (None, Some(text)) => !text.is_empty() && text != markdown,
        (None, None) => false,
    };
    if pending {
        files.entry(id.clone()).or_insert(Synced { stamp: None, text: Some(hash(markdown)), retry: None });
        return;
    }
    if on_disk.as_deref() != Some(markdown)
        && !(on_disk.is_none() && markdown.is_empty())
        && let Err(error) =
            fastctx_platform::publish(&path, markdown.as_bytes(), fastctx_platform::Permissions::Private)
    {
        node.log.line(format_args!("writing the plan of {id} into its file: {error}"));
        return;
    }
    files.insert(id.clone(), Synced { stamp: stamp(&path), text: Some(hash(markdown)), retry: None });
}

/// File synchronization requires current work authority. A persisted isolated
/// host never acquires a local plan during a directory reconnect.
pub(crate) fn allowed(node: &Node, id: &ResidentId) -> Result<bool, fastctx_types::ipc::Failure> {
    let Some((about, _)) = node.residents.about(id) else { return Ok(false) };
    if crate::assists::host::kept(node, id)? == Some(true) {
        return Ok(false);
    }
    node.may_work_here(&about.acting)
}
