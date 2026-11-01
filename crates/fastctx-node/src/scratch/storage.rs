//! One current scratch coordinate per machine, with monotonic delivery receipts.
//! The pending projection is indexed separately, so collecting nothing never
//! scans every machine this conversation has visited.

use crate::records;
use fastctx_types::{
    ResidentId,
    layout::{ConversationDir, WorldDir},
    scratch::Descriptions,
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, io, sync::LazyLock};

#[derive(Clone, Serialize, Deserialize)]
struct Entry {
    revision: u64,
    path: Result<String, String>,
}
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Receipt {
    revisions: BTreeMap<String, u64>,
    #[serde(skip)]
    pub(crate) shown: std::ops::Range<usize>,
    #[serde(skip)]
    pub(crate) reserved: bool,
}
static GATES: LazyLock<crate::record_gates::Gates> = LazyLock::new(|| {
    crate::record_gates::Gates::new(fastctx_platform::doing::Rank::RECORDS, "scratch catalog observation")
});

fn prepare(world: &WorldDir, id: &ResidentId) -> io::Result<ConversationDir> {
    let directory = world.conversation(id);
    if world.deleted().join(format!("{id}.json")).try_exists()?
        || directory.path().join("deleting").try_exists()?
        || !directory.path().try_exists()?
    {
        return Err(io::Error::new(io::ErrorKind::NotFound, "the conversation is unavailable"));
    }
    let path = directory.path().join("scratch-locations.json");
    records::legacy::map_field::<Entry>(&directory, "scratch-known", &path, "known")?;
    records::legacy::map_field::<u64>(&directory, "scratch-seen", &path, "seen")?;
    records::ensure_each(&directory, "scratch-state", |push| {
        if let Some(described) = records::legacy::field::<bool>(&path, "described")? {
            push(records::Source::new("described".into(), path.clone(), serde_json::to_vec(&described)?)?)?;
        }
        Ok(())
    })?;
    records::ensure_each(&directory, "scratch-unseen", |push| {
        let mut after = String::new();
        loop {
            let page = records::read(&directory, |rows| rows.key_page::<Entry>("scratch-known", &after, 16))?;
            let Some(last) = page.last() else { break };
            after = last.0.clone();
            for (machine, entry) in page {
                let seen = records::read(&directory, |rows| rows.get::<u64>("scratch-seen", &machine))?.unwrap_or(0);
                if seen < entry.revision {
                    push(records::Source::new(machine, path.clone(), serde_json::to_vec(&entry)?)?)?;
                }
            }
        }
        Ok(())
    })?;
    Ok(directory)
}

pub(super) fn observe(world: &WorldDir, id: &ResidentId, paths: Descriptions, described: bool) -> io::Result<()> {
    let directory = prepare(world, id)?;
    let gate = GATES.get(directory.path());
    let _observing = gate.lock();
    if described && records::read(&directory, |rows| rows.get::<bool>("scratch-state", "described"))?.unwrap_or(false) {
        return Ok(());
    }
    let mut paths = paths.into_iter();
    loop {
        let group: Vec<_> = paths.by_ref().take(16).collect();
        if group.is_empty() {
            break;
        }
        let bytes = serde_json::to_vec(&group)?.len();
        records::write_at(&directory, fastctx_storage::Priority::Normal, bytes, move |rows| {
            for (machine, path) in group {
                let prior = rows.get::<Entry>("scratch-known", &machine)?;
                let revision = match prior {
                    Some(entry) if entry.path == path => entry.revision,
                    prior => {
                        let revision = prior.map_or(Ok(1), |entry| {
                            entry
                                .revision
                                .checked_add(1)
                                .ok_or_else(|| fastctx_storage::Error::new("scratch revision exhausted"))
                        })?;
                        rows.put("scratch-known", &machine, &Entry { revision, path: path.clone() })?;
                        revision
                    }
                };
                let seen = rows.get::<u64>("scratch-seen", &machine)?.unwrap_or(0);
                if described {
                    rows.put("scratch-seen", &machine, &seen.max(revision))?;
                    rows.delete("scratch-unseen", &machine)?;
                } else if seen < revision {
                    rows.put("scratch-unseen", &machine, &Entry { revision, path })?;
                }
            }
            Ok(())
        })?;
    }
    if described {
        records::write_at(&directory, fastctx_storage::Priority::Normal, 16, |rows| {
            rows.put("scratch-state", "described", &true)
        })?;
    }
    Ok(())
}
pub(super) fn collect(world: &WorldDir, id: &ResidentId, start: usize) -> io::Result<(Vec<String>, Receipt)> {
    let directory = prepare(world, id)?;
    records::read(&directory, |rows| {
        let mut receipt = Receipt::default();
        let mut notes = Vec::new();
        let mut bytes = 0;
        let mut after = String::new();
        'pages: loop {
            let page = rows.key_page::<Entry>("scratch-unseen", &after, 16)?;
            let Some(last) = page.last() else { break };
            after = last.0.clone();
            for (machine, entry) in page {
                let note = fastctx_types::scratch::words::note(&machine, &entry.path);
                if bytes + note.len() > 32 * 1024 && !notes.is_empty() {
                    break 'pages;
                }
                bytes += note.len();
                notes.push(note);
                receipt.revisions.insert(machine, entry.revision);
            }
        }
        receipt.shown = start..start + notes.len();
        Ok((notes, receipt))
    })
}
pub(super) fn acknowledge(world: &WorldDir, id: &ResidentId, receipt: &Receipt) -> io::Result<()> {
    let directory = prepare(world, id)?;
    let mut revisions = receipt.revisions.iter();
    loop {
        let group: Vec<_> = revisions.by_ref().take(16).map(|(key, value)| (key.clone(), *value)).collect();
        if group.is_empty() {
            break;
        }
        records::write_at(&directory, fastctx_storage::Priority::Interactive, 1024, move |rows| {
            for (machine, revision) in group {
                let seen = rows.get::<u64>("scratch-seen", &machine)?.unwrap_or(0).max(revision);
                rows.put("scratch-seen", &machine, &seen)?;
                if rows.get::<Entry>("scratch-known", &machine)?.is_some_and(|entry| entry.revision <= seen) {
                    rows.delete("scratch-unseen", &machine)?;
                }
            }
            Ok(())
        })?;
    }
    Ok(())
}
