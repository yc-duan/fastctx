//! Native Codex threads are not owned by the node's app-server. Resolve their
//! persisted rollout through metadata-only thread/read and follow compactions
//! before collecting context; never resume them or open a model turn.

use crate::Node;
use fastctx_hosts::claude::tail::{Found, Tail};
use fastctx_types::{ResidentId, ipc::Failure};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io,
    path::PathBuf,
    sync::{Arc, Mutex},
};

#[derive(Default)]
pub(crate) struct Observers(Mutex<BTreeMap<ResidentId, Observer>>);

struct Observer {
    path: PathBuf,
    tail: Tail,
    last: Option<String>,
}

#[derive(Default, Serialize, Deserialize)]
struct Observed {
    path: PathBuf,
    last: Option<String>,
    revision: u64,
}

pub(crate) async fn observe(node: &Arc<Node>, id: &ResidentId) -> Result<(), Failure> {
    let Some((about, _)) = node.residents.about(id) else { return Ok(()) };
    // Only a native thread the user brought in is followed: the World reads
    // nothing of a session that is the user's own (`world-layers.md`).
    if about.world_started || !about.native_resumed || about.host.agent.as_str() != "codex" {
        return Ok(());
    }
    let Some(thread) = about.session.as_ref() else { return Ok(()) };
    // A loaded runtime reports compactions directly; its later rollout flush must not reset them twice (2026-10-25).
    if node.codex.of(node, &about)?.loaded(thread) {
        return Ok(());
    }
    let gate = super::working(node, thread);
    let _observing = gate.lock().await;
    let path = node.codex.home(about.home.as_deref())?.rollout(thread).await.map_err(Failure::new)?;
    let Some(path) = path else {
        return Err(Failure::new(crate::context::words::CODEX_PENDING));
    };
    let observer = node.native_context.0.lock().expect("never poisoned").remove(id);
    let (node_read, id_read) = (node.clone(), id.clone());
    let result = tokio::task::spawn_blocking(move || -> io::Result<Observer> {
        let directory = node_read.world.conversation(&id_read);
        let observed_path = directory.path().join("codex-context-observed.json");
        let previous: Option<Observed> = match std::fs::read(&observed_path) {
            Ok(bytes) => Some(serde_json::from_slice(&bytes).map_err(io::Error::other)?),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
        let path_changed = observer.as_ref().is_some_and(|observer| observer.path != path);
        let mut observer = observer.filter(|observer| observer.path == path).unwrap_or_else(|| Observer {
            path: path.clone(),
            tail: Tail::default(),
            last: None,
        });
        let mut rewritten = path_changed;
        observer.tail.read(&path, |found| {
            match found {
                Found::Restarted => {
                    rewritten = true;
                    observer.last = None;
                }
                Found::Row(row) if row["type"] == "compacted" => {
                    observer.last =
                        Some(crate::profile::session::hash(&serde_json::to_string(row).map_err(io::Error::other)?));
                }
                _ => {}
            }
            Ok(())
        })?;
        let changed = rewritten
            || previous.as_ref().is_some_and(|previous| previous.path != path || previous.last != observer.last)
            || (previous.is_none() && directory.path().join("context-state.json").try_exists()?);
        let mut revision = previous.as_ref().map_or(0, |previous| previous.revision);
        if changed {
            revision =
                revision.checked_add(1).ok_or_else(|| io::Error::other(crate::context::words::REVISION_EXHAUSTED))?;
            crate::context::compacted(&node_read, &id_read, &format!("native:{revision}"))?;
            node_read.residents.restore_opening_heard(&id_read);
        }
        if previous.is_none() || changed {
            let _transaction = crate::resident_storage::lock();
            node_read.residents.active(&id_read).map_err(|error| io::Error::other(error.message))?;
            fastctx_platform::publish(
                &observed_path,
                &serde_json::to_vec(&Observed { path: path.clone(), last: observer.last.clone(), revision })
                    .map_err(io::Error::other)?,
                fastctx_platform::Permissions::Private,
            )?;
        }
        Ok(observer)
    })
    .await
    .map_err(|error| Failure::new(error.to_string()))?;
    let observer = result.map_err(|error| Failure::new(crate::context::words::codex_read(error)))?;
    node.native_context.0.lock().expect("never poisoned").insert(id.clone(), observer);
    Ok(())
}

pub(crate) fn forget(node: &Node, id: &ResidentId) {
    node.native_context.0.lock().expect("never poisoned").remove(id);
}
