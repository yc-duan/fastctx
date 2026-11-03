//! The plugin goal and file commit share an operation ID. Recovery finishes a
//! committed goal, or restores its recorded plugin baseline after file rollback.

use super::*;

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct PendingPlugin {
    pub id: String,
    pub plan: PluginApply,
    pub withdrawing: bool,
}

#[derive(Serialize, Deserialize)]
struct Stamp {
    version: u32,
    id: String,
}

pub(super) fn id() -> Result<String, Error> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|error| Error::Io(format!("naming apply operation: {error}")))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

pub(super) fn stamp(places: &Places, id: &str) -> Result<FileChange, Error> {
    let before = Snapshot::read(places.stamp())?;
    let bytes = serde_json::to_vec(&Stamp { version: 1, id: id.into() })
        .map_err(io::Error::other)
        .map_err(io("encoding operation"))?;
    Ok(FileChange::private(before, bytes))
}

fn committed(places: &Places, id: &str) -> Result<bool, Error> {
    let file = Snapshot::read(places.stamp())?;
    let Some(bytes) = file.bytes() else { return Ok(false) };
    let stamp: Stamp = serde_json::from_slice(bytes)
        .map_err(|error| Error::Io(format!("reading {}: {error}", file.path().display())))?;
    if stamp.version != 1 {
        return Err(Error::Io("unsupported operation stamp".into()));
    }
    Ok(stamp.id == id)
}

#[derive(Serialize, Deserialize)]
struct KeptUndo {
    version: u32,
    id: String,
    undo: EntryUndo,
}

/// Keeps what undoes operation `id`'s edit of Claude Code's global state
/// until its files commit, which take it away (`entry_undo_done`).
pub(super) fn keep_entry_undo(places: &Places, id: &str, undo: &EntryUndo) -> Result<(), Error> {
    let bytes = serde_json::to_vec(&KeptUndo { version: 1, id: id.into(), undo: clone_undo(undo) })
        .map_err(io::Error::other)
        .map_err(io("encoding what undoes the global state's edit"))?;
    fastctx_platform::publish(&places.entry_undo(), &bytes, fastctx_platform::Permissions::Private)
        .map_err(io("keeping what undoes the global state's edit"))
}

fn clone_undo(undo: &EntryUndo) -> EntryUndo {
    match undo {
        EntryUndo::Nothing => EntryUndo::Nothing,
        EntryUndo::TakeOut(receipt) => EntryUndo::TakeOut(receipt.clone()),
        EntryUndo::PutBack(receipt) => EntryUndo::PutBack(receipt.clone()),
    }
}

/// The change that takes the kept undo away with the files of its
/// operation, where one is kept.
pub(super) fn entry_undo_done(places: &Places) -> Result<Option<FileChange>, Error> {
    let file = Snapshot::read(places.entry_undo())?;
    Ok(file.exists().then(|| FileChange::delete(file)))
}

/// Lets the kept undo go: it was run, or nothing was edited.
pub(super) fn drop_entry_undo(places: &Places) {
    let _ = std::fs::remove_file(places.entry_undo());
}

/// Runs the kept undo of an operation whose files never committed: an entry
/// it put in is taken out where still as written, one it took out is put
/// back where none stands now; an edit someone made since is left alone.
fn settle_entry_undo(places: &Places, report: &mut Report) -> Result<(), Error> {
    let file = Snapshot::read(places.entry_undo())?;
    let Some(bytes) = file.bytes() else { return Ok(()) };
    let kept: KeptUndo = serde_json::from_slice(bytes)
        .map_err(|error| Error::Io(format!("reading {}: {error}", file.path().display())))?;
    if kept.version != 1 {
        return Err(Error::Io("unsupported global state undo record".into()));
    }
    if !committed(places, &kept.id)? {
        match &kept.undo {
            EntryUndo::Nothing => {}
            EntryUndo::TakeOut(receipt) => report.notices.extend(state::take(receipt, false)?),
            EntryUndo::PutBack(receipt) => state::restore_absent(receipt)?,
        }
    }
    drop_entry_undo(places);
    Ok(())
}

pub(super) fn before(places: &Places) -> Result<Report, Error> {
    let files = crate::transaction::recover(places.journal());
    if places.journal().try_exists().map_err(io("checking pending file recovery"))? {
        return Err(files.err().map(Error::from).unwrap_or_else(|| Error::Io("file recovery is still pending".into())));
    }
    let mut report = Report::default();
    settle_entry_undo(places, &mut report)?;
    let mut records = Vec::new();
    for entry in std::fs::read_dir(&places.root).map_err(io("reading plugin recovery records"))? {
        let entry = entry.map_err(io("reading plugin recovery record"))?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name == "applied-claude-plugin.json"
            || name
                .strip_prefix("applied-claude-plugin-")
                .and_then(|name| name.strip_suffix(".json"))
                .is_some_and(|hash| hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()))
        {
            records.push(entry.path());
        }
    }
    records.sort();
    for record in records {
        let Some(mut previous) = load_plugin_record(&record)? else { continue };
        if previous.settings_path.is_none()
            && record.file_name().is_some_and(|name| name == "applied-claude-plugin.json")
        {
            previous.settings_path = Some(homes::old_plugin_home(places)?.join("settings.json"));
            write_plugin_record(&record, &previous).map_err(Error::Claude)?;
        }
        let Some(pending) = previous.operation.clone() else { continue };
        let settings = previous
            .settings_path
            .clone()
            .ok_or_else(|| Error::Io("pending plugin operation has no Claude profile".into()))?;
        if committed(places, &pending.id)? {
            if pending.withdrawing {
                previous.settings_created = false;
            }
            previous.operation = None;
            write_plugin_record(&record, &previous).map_err(Error::Claude)?;
        } else {
            let claude = places.claude.clone().ok_or(Error::NoClaude)?;
            let mut host = Cli::new(claude, record, settings, previous.settings_created);
            host.operation = previous.operation.clone();
            let receipt = pending.plan.rollback(&mut host, &previous.receipt)?;
            finish_rollback(places, &pending.plan, &receipt, &mut host, &mut report)?;
            report.commands.extend(host.ran);
        }
    }
    let files = files?;
    report.changed.extend(files.changed);
    report.preserved.extend(files.preserved);
    report.leftovers.extend(files.cleanup.iter().map(ToString::to_string));
    if let Err(error) = crate::transaction::collect_finished(&places.root) {
        report.leftovers.push(error.to_string());
    }
    report.leftovers.extend(crate::claude::history::retire(&places.root));
    Ok(report)
}

pub(super) fn finish_rollback(
    places: &Places,
    plan: &PluginApply,
    receipt: &PluginReceipt,
    host: &mut Cli,
    report: &mut Report,
) -> Result<(), Error> {
    // A file rollback may have reinstated the post-command settings image.
    // Reconcile the reserved declarations after that rollback, from fresh bytes.
    if places.journal().try_exists().map_err(io("checking file rollback"))? {
        return Err(Error::Io(format!(
            "file rollback remains pending at {}; the plugin baseline is recorded",
            places.journal().display()
        )));
    }
    cleanup_registration(places, host, !plan.kept_plugin(), !plan.kept_marketplace(), report)?;
    if !plan.kept_plugin() && !plan.kept_marketplace() {
        host.settings_created = false;
    }
    host.operation = None;
    host.checkpoint(receipt).map_err(Error::Claude)
}

fn cleanup_registration(
    places: &Places,
    host: &Cli,
    plugin: bool,
    marketplace: bool,
    report: &mut Report,
) -> Result<(), Error> {
    let snapshot = Snapshot::read(&host.settings_path)?;
    let mut bytes =
        crate::claude::registration::remove(snapshot.bytes(), &ClaudePlan::new(&places.world), plugin, marketplace)
            .map_err(Error::Claude)?;
    if host.settings_created && plugin && marketplace {
        bytes = settings::discard_empty_created(bytes).map_err(Error::Claude)?;
    }
    let mut changes = vec![];
    change(&mut changes, snapshot, bytes);
    if changes.is_empty() {
        return Ok(());
    }
    let receipt = Transaction::new(changes)
        .commit_durable(places.journal())
        .map_err(|failure| Error::Files(failure.to_string()))?;
    report.changed.extend(receipt.changed);
    report.preserved.extend(receipt.preserved);
    report.leftovers.extend(receipt.cleanup.iter().map(ToString::to_string));
    Ok(())
}
