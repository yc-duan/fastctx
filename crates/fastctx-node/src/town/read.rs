//! A town's ledger read as files: the versions of its record, its journals
//! and their entries.

use std::io;
use std::path::{Path, PathBuf};

use fastctx_types::layout::WorldDir;
use fastctx_types::project::{Description, EntryStamp, JournalSummary, Record, Writer, version_of};
use fastctx_types::{EntryId, JournalName, TownName};

/// Every version of the town's record, oldest first.
pub fn records(world: &WorldDir, town: &TownName) -> io::Result<Vec<Record>> {
    versions(&world.town(town).ledger().join("project"))
}

/// The town's record as its latest version says; `None` when this machine
/// keeps no ledger of the town.
pub fn latest(world: &WorldDir, town: &TownName) -> io::Result<Option<Record>> {
    Ok(records(world, town)?.pop())
}

/// The journals of a town, by name.
pub fn journal_names(world: &WorldDir, town: &TownName) -> io::Result<Vec<JournalName>> {
    let mut names: Vec<JournalName> = read_dir(&journals_dir(world, town))?
        .into_iter()
        .filter(|path| path.is_dir())
        .filter_map(|path| JournalName::try_from(path.file_name()?.to_str()?).ok())
        .collect();
    names.sort();
    Ok(names)
}

pub fn journal_dir(world: &WorldDir, town: &TownName, journal: &JournalName) -> PathBuf {
    journals_dir(world, town).join(journal.as_str())
}

fn journals_dir(world: &WorldDir, town: &TownName) -> PathBuf {
    world.town(town).ledger().join("journals")
}

/// Every version of a journal's description, oldest first.
pub fn descriptions(world: &WorldDir, town: &TownName, journal: &JournalName) -> io::Result<Vec<Description>> {
    versions(&journal_dir(world, town, journal).join("description"))
}

/// Every entry of a journal in the order they were written, with the
/// directory holding each.
pub fn entries(world: &WorldDir, town: &TownName, journal: &JournalName) -> io::Result<Vec<(EntryStamp, PathBuf)>> {
    let mut entries = Vec::new();
    for directory in read_dir(&journal_dir(world, town, journal).join("entries"))? {
        let stamp_file = directory.join("entry.json");
        // An entry's files come with the entry, so one without its stamp is
        // not an entry.
        if !stamp_file.exists() {
            continue;
        }
        entries.push((read_json::<EntryStamp>(&stamp_file)?, directory));
    }
    entries.sort_by(|(a, _), (b, _)| (a.at, &a.id).cmp(&(b.at, &b.id)));
    Ok(entries)
}

/// An entry's text.
pub fn text(directory: &Path) -> io::Result<String> {
    std::fs::read_to_string(directory.join("text.md"))
}

/// One journal in brief.
pub fn summary(world: &WorldDir, town: &TownName, journal: &JournalName) -> io::Result<JournalSummary> {
    let entries = entries(world, town, journal)?;
    let stamps: Vec<EntryStamp> = entries.into_iter().map(|(stamp, _)| stamp).collect();
    Ok(JournalSummary {
        journal: journal.clone(),
        description: descriptions(world, town, journal)?.pop(),
        entries: stamps.len(),
        writers: writers(stamps.iter().map(|stamp| &stamp.by)),
        latest: stamps.last().cloned(),
    })
}

/// Every journal of a town in brief, the latest written first.
pub fn summaries(world: &WorldDir, town: &TownName) -> io::Result<Vec<JournalSummary>> {
    let mut summaries = journal_names(world, town)?
        .iter()
        .map(|journal| summary(world, town, journal))
        .collect::<io::Result<Vec<_>>>()?;
    summaries.sort_by_key(|summary| {
        let written = summary
            .latest
            .as_ref()
            .map(|stamp| stamp.at)
            .or(summary.description.as_ref().map(|description| description.at));
        std::cmp::Reverse(written)
    });
    Ok(summaries)
}

/// The journal holding the entry `id`, when the town has it.
pub fn find_entry(world: &WorldDir, town: &TownName, id: &EntryId) -> io::Result<Option<JournalName>> {
    for journal in journal_names(world, town)? {
        if journal_dir(world, town, &journal).join("entries").join(id.as_str()).join("entry.json").exists() {
            return Ok(Some(journal));
        }
    }
    Ok(None)
}

/// How many different writers.
pub fn writers<'a>(writers: impl Iterator<Item = &'a Writer>) -> usize {
    let mut seen: Vec<&Writer> = Vec::new();
    for writer in writers {
        let same = |other: &&Writer| match (*other, writer) {
            (Writer::Page { page: a }, Writer::Page { page: b }) => a.town == b.town && a.page == b.page,
            _ => *other == writer,
        };
        if !seen.iter().any(same) {
            seen.push(writer);
        }
    }
    seen.len()
}

/// The versions in a directory of `v<number>.json` files, oldest first.
fn versions<T: serde::de::DeserializeOwned>(directory: &Path) -> io::Result<Vec<T>> {
    let mut found: Vec<(u32, PathBuf)> = read_dir(directory)?
        .into_iter()
        .filter_map(|path| Some((version_of(path.file_name()?.to_str()?)?, path)))
        .collect();
    found.sort();
    found.into_iter().map(|(_, path)| read_json(&path)).collect()
}

fn read_dir(path: &Path) -> io::Result<Vec<PathBuf>> {
    match std::fs::read_dir(path) {
        Ok(entries) => entries.map(|entry| entry.map(|entry| entry.path())).collect(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error),
    }
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> io::Result<T> {
    serde_json::from_slice(&std::fs::read(path)?)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, format!("{}: {error}", path.display())))
}
