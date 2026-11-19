//! Same-file openers share one process-local attempt, including startup in flight.
use super::{MigrationGate, gate::WeakGate};
use crate::Result;
use fastctx_platform::rooted_path::{FileIdentity, path_identity};
use std::{
    collections::HashMap,
    path::Path,
    sync::{LazyLock, Mutex},
    time::Duration,
};
static RUNS: LazyLock<Mutex<HashMap<FileIdentity, WeakGate>>> = LazyLock::new(Default::default);
/// Never by opening the database's file: closing that descriptor would drop
/// this process's locks on a database it has open (`path_identity`).
fn key(path: &Path) -> Result<FileIdentity> {
    Ok(path_identity(path)?)
}
pub(super) fn claim(path: &Path, elapsed: Duration) -> Result<(MigrationGate, bool)> {
    let id = key(path)?;
    let mut runs = RUNS.lock().expect("format backup registry");
    runs.retain(|_, run| run.upgrade().is_some());
    if let Some(gate) = runs.get(&id).and_then(WeakGate::upgrade)
        && !gate.needs_resume()
    {
        return Ok((gate, false));
    }
    let gate = MigrationGate::new(elapsed);
    runs.insert(id, gate.downgrade());
    Ok((gate, true))
}
/// Cancels this process's backup before a lifecycle owner removes the database.
/// A Busy result means handles are still live and deletion must be deferred.
pub fn stop_format_backup(path: &Path, within: Duration) -> Result<()> {
    let began = std::time::Instant::now();
    let id = match key(path) {
        Ok(id) => id,
        Err(_) if !path.try_exists()? => return Ok(()),
        Err(e) => return Err(e),
    };
    let gate = RUNS.lock().expect("format backup registry").get(&id).and_then(WeakGate::upgrade);
    if let Some(gate) = gate {
        gate.drain(within)?;
    }
    crate::keeper::wait_closed(path, within.saturating_sub(began.elapsed()))?;
    Ok(())
}
