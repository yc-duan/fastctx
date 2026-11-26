//! Scratch coordinates come from the machine that owns the directory. The
//! same layout lookup used by run preserves guest rooms across host renames.

pub(crate) mod jobs;
pub(crate) mod storage;
pub(crate) use storage::Receipt;

use crate::Node;
use fastctx_types::{
    Address, ResidentId,
    access::{Acting, Purpose, Written},
    remote::{RemoteCall, RemoteReply},
    scratch::{Descriptions, words},
};
use std::{io, sync::Arc};

pub(crate) fn here(node: &Node, owner: &Address) -> String {
    let path = node.world.owner(&node.names(), owner).scratch();
    fastctx_hands::run::present::display_path(&path.to_string_lossy())
}

/// How many machines are asked where a scratch directory is at once, as
/// other fan-outs ask (`fastctx_net::around`).
const ASKING_AT_ONCE: usize = 32;

/// How long a machine's answer stands for a conversation's listing of its
/// tools: a session started again, or its tools listed again, does not ask
/// every machine again (`descriptions`). Not longer: a scratch directory
/// left unused is cleared there in time, and made again when asked.
const KNOWN_FOR: std::time::Duration = std::time::Duration::from_secs(30 * 60);

/// Each conversation's scratch directory on each machine, as it answered,
/// and when.
type Known = std::collections::HashMap<(ResidentId, String), (String, std::time::Instant)>;
static KNOWN: std::sync::LazyLock<std::sync::Mutex<Known>> = std::sync::LazyLock::new(Default::default);

fn known(id: &ResidentId, machine: &str) -> Option<String> {
    let known = KNOWN.lock().expect("never poisoned");
    known
        .get(&(id.clone(), machine.to_string()))
        .filter(|(_, at)| at.elapsed() < KNOWN_FOR)
        .map(|(path, _)| path.clone())
}

/// How long one machine is given to say, waiting for its turn to be asked
/// and for its hub included: it is then told as one that gave no answer, and
/// its path comes with its first reply (`observed`). They are asked for a
/// session's listing of its tools, which a host waits for 30 seconds at most
/// and the listing waits for the node 10 (`fastctx-serve`): one machine slow
/// to say must not hold up the rest of it.
const ANSWERING: std::time::Duration = std::time::Duration::from_secs(5);

pub(crate) async fn descriptions(
    node: &Arc<Node>,
    id: &ResidentId,
    acting: &Acting,
    machines: Vec<String>,
) -> Descriptions {
    let mut pending = tokio::task::JoinSet::new();
    let mut tasks = std::collections::HashMap::new();
    let asking = Arc::new(tokio::sync::Semaphore::new(ASKING_AT_ONCE));
    for machine in machines {
        let (node, id, acting, asking) = (node.clone(), id.clone(), acting.clone(), asking.clone());
        let name = machine.clone();
        let task = pending.spawn(async move {
            let what = format!("{machine}'s scratch directory");
            fastctx_platform::doing::outside(what, ANSWERING, async {
                let _turn = asking.acquire_owned().await.map_err(|error| error.to_string())?;
                let written = Written::parse(&machine).map_err(|error| error.to_string())?;
                let owner = Address { machine: node.machine(), resident: id };
                if node.is_here(&written) {
                    node.authorize_here(&acting).map_err(|error| error.message)?;
                    return Ok(here(&node, &owner));
                }
                let (link, account) =
                    node.speaking(&acting, &written, Purpose::Hands).await.map_err(|error| error.to_string())?;
                if !link.view().scratch_locations {
                    return Err(words::OLD_HUB.into());
                }
                if !link.view().member(&written.name).is_some_and(|member| member.online) {
                    return Err(words::OFFLINE.into());
                }
                // May still be used, and online: as it answered lately.
                if let Some(path) = known(&owner.resident, &machine) {
                    return Ok(path);
                }
                let resident = owner.resident.clone();
                match link
                    .call(&written.name, &account, Purpose::Hands, RemoteCall::Scratch { owner })
                    .await
                    .map_err(|error| error.to_string())?
                    .map_err(|error| error.message)?
                {
                    RemoteReply::Scratch { path } => {
                        let key = (resident, machine.clone());
                        KNOWN.lock().expect("never poisoned").insert(key, (path.clone(), std::time::Instant::now()));
                        Ok(path)
                    }
                    _ => Err(words::WRONG_REPLY.into()),
                }
            })
            .await
            .map_err(|late| late.to_string())?
        });
        tasks.insert(task.id(), name);
    }
    let mut descriptions = Descriptions::new();
    while let Some(result) = pending.join_next_with_id().await {
        let (task, path) = match result {
            Ok((task, path)) => (task, path),
            Err(error) => (error.id(), Err(error.to_string())),
        };
        descriptions.insert(tasks.remove(&task).expect("spawned scratch lookup"), path);
    }
    descriptions
}

#[derive(Default)]
pub(crate) struct Deliveries(std::sync::Mutex<std::collections::BTreeSet<ResidentId>>);

pub(crate) fn described(node: &Node, id: &ResidentId, paths: Descriptions) -> io::Result<()> {
    storage::observe(&node.world, id, paths, true)
}

/// Bind a direct executor's location to the name used for that call. Descendant
/// locations carry their hub identity so equal names remain distinguishable.
pub(crate) fn observed(
    node: &Node,
    id: &ResidentId,
    source: &Written,
    locations: &[fastctx_types::scratch::Location],
) -> io::Result<()> {
    let mut paths = Descriptions::new();
    if locations.is_empty() {
        paths.insert(source.to_string(), Err(words::UNREPORTED.into()));
    }
    for location in locations {
        let name = match &location.hub {
            None => source.to_string(),
            Some(hub) => {
                let link =
                    node.link_of(hub).ok_or_else(|| io::Error::other("scratch location names an unknown hub"))?;
                node.written(&link, &location.machine).to_string()
            }
        };
        paths.insert(name, location.path.clone());
    }
    storage::observe(&node.world, id, paths, false)
}

pub(crate) fn collect(node: &Node, id: &ResidentId, start: usize) -> io::Result<(Vec<String>, Receipt)> {
    storage::collect(&node.world, id, start)
}

pub(crate) fn reserve(
    node: &Node,
    id: &ResidentId,
    heard: &mut crate::residents::Heard,
) -> Option<std::ops::Range<usize>> {
    let receipt = heard.scratch.as_mut()?;
    if receipt.shown.is_empty() {
        return None;
    }
    if !node.scratch_deliveries.0.lock().expect("never poisoned").insert(id.clone()) {
        let shown = receipt.shown.clone();
        heard.scratch = None;
        return Some(shown);
    }
    receipt.reserved = true;
    None
}

pub(crate) fn release(node: &Node, id: &ResidentId, heard: &crate::residents::Heard) {
    if heard.scratch.as_ref().is_some_and(|receipt| receipt.reserved) {
        node.scratch_deliveries.0.lock().expect("never poisoned").remove(id);
    }
}

pub(crate) fn acknowledge(node: &Node, id: &ResidentId, heard: &mut crate::residents::Heard) -> io::Result<()> {
    if let Some(receipt) = &heard.scratch {
        storage::acknowledge(&node.world, id, receipt)?;
    }
    release(node, id, heard);
    if let Some(receipt) = &mut heard.scratch {
        receipt.reserved = false;
    }
    Ok(())
}
