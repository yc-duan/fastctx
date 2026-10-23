//! Native host lifetimes survive node restart; an absent connection is not an exit.
use fastctx_types::{ProcessIdentity, ResidentId, layout::WorldDir};
use serde::{Deserialize, Serialize};
use std::io;

#[derive(Default, Serialize, Deserialize)]
struct Hosts {
    processes: Vec<ProcessIdentity>,
    unknown: bool,
}
fn path(world: &WorldDir, id: &ResidentId) -> std::path::PathBuf {
    world.conversation(id).path().join("native-hosts.json")
}

pub(crate) fn record(world: &WorldDir, id: &ResidentId, process: Option<ProcessIdentity>) -> io::Result<()> {
    let path = path(world, id);
    let prior: Hosts = load(&path).map_err(io::Error::other)?.unwrap_or_default();
    let ended: Vec<_> =
        prior.processes.into_iter().filter(|p| matches!(fastctx_platform::has_ended(p), Ok(true))).collect();
    let _transaction = crate::resident_storage::lock();
    let mut hosts: Hosts = load(&path).map_err(io::Error::other)?.unwrap_or_default();
    hosts.processes.retain(|p| !ended.contains(p));
    hosts.unknown = process.is_none();
    if let Some(process) = process
        && !hosts.processes.contains(&process)
    {
        hosts.processes.push(process);
    }
    fastctx_platform::ensure_private_dir(path.parent().expect("native lifetime directory"))?;
    fastctx_platform::publish(&path, &serde_json::to_vec(&hosts)?, fastctx_platform::Permissions::Private)
}
/// Missing or unreadable lifetime evidence is an explicit unavailable result.
pub(crate) fn processes(world: &WorldDir, id: &ResidentId) -> Result<Vec<ProcessIdentity>, String> {
    let hosts: Hosts = load(&path(world, id))?.ok_or(
        "the native host lifetime is unknown; reconnect with updated World tools before background resumption",
    )?;
    if hosts.unknown || hosts.processes.is_empty() {
        return Err(
            "the native host lifetime is unknown; reconnect with updated World tools before background resumption"
                .into(),
        );
    }
    Ok(hosts.processes)
}
pub(crate) fn closed(world: &WorldDir, id: &ResidentId, controlled: Option<&ProcessIdentity>) -> Result<bool, String> {
    for process in processes(world, id)? {
        if controlled == Some(&process) {
            continue;
        }
        if !fastctx_platform::has_ended(&process).map_err(|e| format!("checking the native host lifetime: {e}"))? {
            return Ok(false);
        }
    }
    Ok(true)
}

fn load(path: &std::path::Path) -> Result<Option<Hosts>, String> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map(Some).map_err(|error| error.to_string()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}
