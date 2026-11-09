//! A carry keeps one translation coordinate across retries and machine renames.

use super::*;

pub(super) async fn machine(
    node: &Node,
    link: &Link,
    town: &TownName,
    journal: &mut Journal,
    entries: &[(u64, Entry, Signed)],
    selected: &[usize],
    own: &fastctx_net::sign::PublicKey,
) -> Result<MachineName, String> {
    if let Some(machine) = &journal.translated_on {
        return Ok(machine.clone());
    }
    // Older journals did not retain the coordinate. Adopt one only after the
    // whole accepted prefix verifies under it; the prefix itself stays intact.
    let (page, _) = link.ledger_read(&journal.target.town, 0).await?;
    let mut candidates = Vec::new();
    if let Some(entry) = page.first().and_then(|item| item.entry.as_ref()) {
        candidates.push(entry.author.clone());
    }
    candidates.push(node.machine());
    candidates
        .extend(entries.iter().filter(|(_, entry, _)| entry.key == *own).map(|(_, entry, _)| entry.author.clone()));
    let mut tried = std::collections::BTreeSet::new();
    let mut failure = None;
    for machine in candidates {
        if !tried.insert(machine.clone()) {
            continue;
        }
        match prefix(&machine, link, &journal.target.town, entries, selected, own).await {
            Ok(_) => {
                journal.translated_on = Some(machine.clone());
                keep(&node.world, town, journal).map_err(|error| error.to_string())?;
                return Ok(machine);
            }
            Err(error) => failure = Some(error),
        }
    }
    Err(failure.unwrap_or_else(|| "the carried ledger has no translation coordinate".into()))
}

pub(super) fn translated(
    here: &MachineName,
    link: &Link,
    entry: &Entry,
    own: &fastctx_net::sign::PublicKey,
) -> Result<Entry, String> {
    let old = &entry.author;
    let ours = entry.key == *own && old != here;
    let renamed = |name: &MachineName| if ours && name == old { here.clone() } else { name.clone() };
    let mut files = Vec::new();
    for file in &entry.files {
        let bytes = if ours {
            Bytes(crate::town::rename::file(&file.path, &file.bytes.0, old, here)?)
        } else {
            file.bytes.clone()
        };
        files.push(File { path: file.path.clone(), bytes });
    }
    if ours {
        crate::town::rename::memory(&mut files, old, here)?;
    }
    history::stamp(entry, &mut files)?;
    let mut carried =
        link.entry(&entry.town, entry.machines.as_ref().map(|machines| machines.iter().map(renamed).collect()), files);
    carried.at = entry.at;
    Ok(carried)
}
