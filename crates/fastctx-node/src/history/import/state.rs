//! Import destinations and the final archival batch survive every network attempt.

use super::archive::{self, Original};
use fastctx_net::link::state;
use fastctx_platform::disk;
use fastctx_types::{access::HubAccount, history::stored::Stored};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io,
    path::{Path, PathBuf},
};

#[derive(Serialize, Deserialize)]
pub(super) struct Transfer {
    pub account: HubAccount,
    pub opened: jiff::Timestamp,
    pub names: BTreeMap<String, String>,
    pub sources: Vec<Original>,
    #[serde(default)]
    pub conversation: Option<fastctx_types::owned_records::Conversation>,
    #[serde(default)]
    pub skipped: BTreeMap<String, String>,
    #[serde(default)]
    pub originals: Option<Vec<Original>>,
    #[serde(default)]
    pub done: bool,
}

fn path(records: &Path) -> PathBuf {
    records.parent().expect("conversation history parent").join("history-import.json")
}

pub(super) fn read(records: &Path) -> io::Result<Option<Transfer>> {
    state::read_or(&path(records), None)
}

fn live<T>(records: &Path, work: impl FnOnce() -> io::Result<T>) -> io::Result<T> {
    let directory = fastctx_types::layout::ConversationDir::at(records.parent().expect("conversation history parent"));
    fastctx_platform::roles::in_place("updating a history import", || {
        crate::conversation_store::with_live_files(&directory, work)
    })
}

pub(super) fn keep(records: &Path, transfer: &Transfer) -> io::Result<()> {
    live(records, || crate::record_migration::archive::keep_import(&path(records), transfer))
}

pub(super) fn finish(records: &Path, transfer: &mut Transfer) -> io::Result<()> {
    live(records, || {
        for original in transfer.originals.as_ref().expect("archival prepared") {
            original.finish()?;
        }
        if transfer.skipped.is_empty() {
            archive::empty(records)?;
        }
        transfer.done = true;
        transfer.originals = None;
        transfer.sources.clear();
        crate::record_migration::archive::keep_import(&path(records), transfer)
    })
}

pub(crate) fn pending(records: &Path) -> bool {
    match read(records) {
        Ok(Some(transfer)) => !transfer.done,
        Ok(None) => disk::is_there(records),
        Err(_) => true,
    }
}

pub(crate) fn belongs_to_account(records: &Path) -> io::Result<bool> {
    read(records).map(|transfer| transfer.is_some())
}

pub(super) fn inventory(records: &Path) -> io::Result<BTreeMap<String, Vec<(u64, PathBuf)>>> {
    let mut found = BTreeMap::new();
    let folders = match disk::read_dir(records) {
        Ok(folders) => folders,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(found),
        Err(error) => return Err(error),
    };
    for folder in folders {
        let folder = folder?;
        if !folder.file_type()?.is_dir() {
            continue;
        }
        let name = folder.file_name().to_string_lossy().to_string();
        fastctx_types::history::markdown::check_name(&name).map_err(io::Error::other)?;
        let mut versions = Vec::new();
        for entry in disk::read_dir(folder.path())? {
            let path = entry?.path();
            if path.extension().is_none_or(|extension| extension != "json") {
                continue;
            }
            let version = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .and_then(|stem| stem.parse::<u64>().ok())
                .filter(|version| *version > 0)
                .ok_or_else(|| io::Error::other("invalid standalone history version filename"))?;
            versions.push((version, path));
        }
        versions.sort_by_key(|(version, _)| *version);
        if !versions.is_empty() {
            found.insert(name, versions);
        }
    }
    Ok(found)
}

pub(super) fn stored(path: &Path, name: &str, version: u64) -> Result<Stored, String> {
    let stored: Stored = state::read(path).map_err(|error| error.to_string())?;
    stored.validate()?;
    if stored.revision.name != name || stored.revision.version != version {
        return Err("the standalone history filename differs from its revision".into());
    }
    Ok(stored)
}

/// Local readers retain the complete source while a partially finished archive resumes.
pub(crate) fn archiving(records: &Path) -> Result<Option<Vec<PathBuf>>, String> {
    let Some(transfer) = read(records).map_err(|error| error.to_string())? else { return Ok(None) };
    let Some(originals) = transfer.originals.filter(|_| !transfer.done) else { return Ok(None) };
    let mut paths: BTreeMap<PathBuf, PathBuf> = inventory(records)
        .map_err(|error| error.to_string())?
        .into_values()
        .flatten()
        .map(|(_, path)| (path.clone(), path))
        .collect();
    for original in originals {
        paths.insert(original.source().into(), original.readable().into());
    }
    Ok(Some(paths.into_values().collect()))
}

pub(crate) fn readable(path: &Path) -> Result<PathBuf, String> {
    if disk::is_there(path) {
        return Ok(path.into());
    }
    let records = path.parent().and_then(Path::parent).ok_or("invalid history source path")?;
    let transfer = read(records).map_err(|error| error.to_string())?;
    Ok(transfer
        .and_then(|transfer| transfer.originals)
        .and_then(|originals| originals.into_iter().find(|original| original.source() == path))
        .map_or_else(|| path.into(), |original| original.readable().into()))
}
