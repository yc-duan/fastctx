//! The readable Markdown copies are projections, never writable authorities.

use std::collections::BTreeMap;
use std::io;

use super::{
    copy::{self, Content},
    store,
};
use fastctx_types::{TownName, layout::WorldDir};

pub(crate) fn restore(world: &WorldDir, town: &TownName) -> io::Result<Vec<String>> {
    if !world.town(town).signed().is_dir() {
        return Ok(Vec::new());
    }
    let store = store::read(world, town)?;
    let catalog = store.index.catalog();
    let directory = world.town(town);
    let mut journals = BTreeMap::new();
    let mut specs = BTreeMap::new();
    for revision in catalog.journals.iter().chain(&catalog.specs) {
        let bytes = store
            .files
            .iter()
            .find(|file| file.path == revision.path("md"))
            .ok_or_else(|| io::Error::other(fastctx_types::memory::words::REVISION_NO_MARKDOWN))?
            .bytes
            .0
            .clone();
        let target = match revision.kind {
            fastctx_types::memory::Kind::Journal => &mut journals,
            fastctx_types::memory::Kind::Spec => &mut specs,
        };
        target.insert(format!("{}.md", revision.name), Content::Bytes(bytes));
    }
    for journal in &catalog.journals {
        for evidence in store.index.evidence(&journal.name) {
            let path = format!("{}/{}", journal.name, evidence.name);
            let content = if let Some(location) = &evidence.location {
                if !crate::machine::names(world)?.is(&location.machine) {
                    continue;
                }
                let source = directory.attachments().join("memory").join(&journal.name).join(&evidence.sha256);
                if !source.is_file() {
                    return Err(io::Error::other(fastctx_types::memory::words::missing_evidence(source.display())));
                }
                Content::File { source, size: evidence.bytes, hash: evidence.sha256 }
            } else {
                let bytes = store
                    .files
                    .iter()
                    .find(|file| file.path == evidence.content_path())
                    .ok_or_else(|| io::Error::other(fastctx_types::memory::words::EVIDENCE_MISSING))?
                    .bytes
                    .0
                    .clone();
                Content::Bytes(bytes)
            };
            journals.insert(path, content);
        }
    }
    let at = jiff::Timestamp::now().strftime("%Y%m%dT%H%M%S%.fZ").to_string();
    let archive = directory.path().join("tampering").join(format!("memory-{at}"));
    let mut changes = Vec::new();
    for (folder, expected) in [("journals", journals), ("specs", specs)] {
        changes.extend(
            copy::reconcile(&directory.path().join(folder), &expected, &archive.join(folder))?
                .into_iter()
                .map(|path| format!("{folder}/{path}")),
        );
    }
    if !changes.is_empty() {
        fastctx_platform::ensure_private_dir(&archive)?;
        fastctx_net::link::state::write(&archive.join("restored.json"), &changes)?;
    }
    Ok(changes)
}
