//! A conversation's plan is a Markdown file in its directory on the machine it
//! lives on (`world-plan.md`): the model changes it as it changes any file,
//! and each change saved becomes the plan's next version on the hub, where the
//! interface, the other devices and the colleagues read it. A version someone
//! else wrote there, as the user from the plan's panel, comes back into the
//! file, unless the file holds changes not taken yet: then the file wins. The
//! node looks at the files of the conversations it holds every two seconds.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use fastctx_types::ResidentId;
use fastctx_types::plan::Page;
use fastctx_types::residents::Origin;

use crate::Node;

const EVERY: Duration = Duration::from_secs(2);

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
pub(crate) struct PlanFiles(Mutex<HashMap<ResidentId, Synced>>);

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

/// Looks at the plan files of the conversations held here, for as long as
/// the node runs.
pub(crate) fn start(node: &Arc<Node>) {
    let node = Arc::downgrade(node);
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(EVERY).await;
            let Some(node) = node.upgrade() else { return };
            look(&node).await;
        }
    });
}

/// Takes each plan file changed since it was last looked at.
async fn look(node: &Arc<Node>) {
    let held: Vec<ResidentId> = node
        .residents
        .everyone()
        .filter(|held| {
            held.here
                && held.removed.is_none()
                && held.about.world_managed()
                && matches!(held.about.origin, Origin::User)
        })
        .map(|held| held.id)
        .collect();
    for id in held {
        let path = path(node, &id);
        let now = stamp(&path);
        let before = node.plan_files.0.lock().expect("never poisoned").get(&id).copied();
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
                Err(_) => continue,
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
    if !about.world_managed() || !matches!(about.origin, Origin::User) {
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
