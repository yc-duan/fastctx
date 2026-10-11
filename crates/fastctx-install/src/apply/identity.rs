//! Upgrade owned Codex servers without changing guidance or native tools.

use super::*;

/// Forwards the host's selected login directory, without embedding a local
/// path in configurations shared across machines. Edited servers are reported.
pub fn forward_login_identity(places: &Places) -> Result<Report, Error> {
    let _lock = FileLock::acquire(&places.lock()).map_err(io("taking the apply lock"))?;
    if places.journal().try_exists().map_err(io("checking pending apply"))? {
        return Err(Error::Io("login identity upgrade waits for the pending apply operation to complete".into()));
    }
    let snapshot = Snapshot::read(places.record())?;
    let mut records = homes::Records::read(&snapshot, places)?;
    let mut report = Report::default();
    let mut changes = Vec::new();
    for integration in records.integrations().into_iter().filter(|entry| entry.host == Host::Codex) {
        let selected = places.at_home(Host::Codex, &integration.home)?;
        let mut record = records.selected(&selected, Host::Codex.only())?;
        let Some(receipt) = record.codex.as_ref() else { continue };
        let config = Snapshot::read(selected.codex_home.join("config.toml"))?;
        let path = text(config.path());
        match codex::forward_home(File { path: &path, bytes: config.bytes() }, receipt) {
            Ok(Some((bytes, receipt))) => {
                changes.push(FileChange::write(config, bytes));
                record.codex = Some(receipt);
                records.update(&selected, Host::Codex.only(), &record)?;
            }
            Ok(None) => {}
            Err(error) => report.notices.push(Notice { location: path, reason: error.to_string() }),
        }
    }
    if !changes.is_empty() {
        let bytes = serde_json::to_vec(&records).map_err(|error| Error::Io(error.to_string()))?;
        changes.push(FileChange::private(snapshot, bytes));
        let committed = Transaction::new(changes)
            .commit_durable(places.journal())
            .map_err(|failure| Error::Files(failure.to_string()))?;
        report.changed.extend(committed.changed);
        report.preserved.extend(committed.preserved);
        report.leftovers.extend(committed.cleanup.iter().map(ToString::to_string));
    }
    Ok(report)
}
