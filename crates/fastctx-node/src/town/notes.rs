//! What a resident is told of where it lives, in the head of the answers of
//! the World's tools (`world-projects.md`, `world-map.md`): who and where it
//! is, the first time; and what its town gained since it last heard,
//! without sending the map again.

use std::collections::BTreeMap;
use std::io;

use fastctx_types::layout::WorldDir;
use fastctx_types::project::{EntryStamp, Record, Writer};
use fastctx_types::{Address, JournalName, MachineName, ResidentId, TownName};

use crate::Node;
use crate::residents::Heard;

/// The lines for the head of the resident `id`'s next answer.
pub fn notes(node: &Node, id: &ResidentId) -> io::Result<(Vec<String>, Option<Heard>)> {
    let here = node.machine();
    let Some(town) = node.residents.town_of(id) else { return Ok((Vec::new(), None)) };
    let ledger = match &town {
        Some(town) => crate::ledger::seq(&node.world, town)?,
        None => 0,
    };
    let now = Heard { town: town.clone(), ledger };
    let notes = match node.residents.heard(id) {
        None => vec![introduction(id, &here, town.as_ref())],
        // It moved: entering gave it the new town's map.
        Some(heard) if heard.town != town => Vec::new(),
        Some(heard) if heard.ledger >= ledger => return Ok((Vec::new(), None)),
        Some(heard) => {
            let me = Address { machine: here.clone(), resident: id.clone() };
            match &town {
                Some(town) => news(&node.world, town, heard.ledger, &me)?.into_iter().collect(),
                None => Vec::new(),
            }
        }
    };
    Ok((notes, Some(now)))
}

fn introduction(id: &ResidentId, here: &MachineName, town: Option<&TownName>) -> String {
    match town {
        Some(town) => {
            format!("=== you are {id} on {here}, living in the town {town}; its map: fastctx project map ===")
        }
        None => format!(
            "=== you are {id} on {here}, in no town yet. When the user names the project to work on, enter its town: fastctx project enter <town> (fastctx project list lists them) ==="
        ),
    }
}

/// One line saying what the town's ledger gained after `after`, leaving
/// out what `me` wrote; `None` when nothing else came.
fn news(world: &WorldDir, town: &TownName, after: u64, me: &Address) -> io::Result<Option<String>> {
    let mine = |by: &Writer| matches!(by, Writer::Resident { address } if address == me);
    let ledger = world.town(town).ledger();
    let mut entries: BTreeMap<JournalName, usize> = BTreeMap::new();
    let mut started: Vec<JournalName> = Vec::new();
    let mut described: Vec<JournalName> = Vec::new();
    let mut record: Option<u32> = None;
    for (_, entry) in crate::ledger::entries_after(world, town, after)? {
        for file in &entry.files {
            let parts: Vec<&str> = file.path.split('/').collect();
            match parts.as_slice() {
                ["project", _] => {
                    let version: Record = parse(&file.bytes.0, &ledger.join(&file.path))?;
                    if !mine(&version.by) {
                        record = record.max(Some(version.version));
                    }
                }
                ["journals", journal, "entries", _, "entry.json"] => {
                    let stamp: EntryStamp = parse(&file.bytes.0, &ledger.join(&file.path))?;
                    if let Ok(journal) = JournalName::try_from(*journal)
                        && !mine(&stamp.by)
                    {
                        *entries.entry(journal).or_default() += 1;
                    }
                }
                ["journals", journal, "description", version] => {
                    let description: fastctx_types::project::Description =
                        parse(&file.bytes.0, &ledger.join(&file.path))?;
                    if let Ok(journal) = JournalName::try_from(*journal)
                        && !mine(&description.by)
                    {
                        if *version == "v000001.json" { started.push(journal) } else { described.push(journal) }
                    }
                }
                _ => {}
            }
        }
    }
    let mut said = Vec::new();
    for journal in &started {
        said.push(format!("a journal {journal} was started"));
    }
    for (journal, count) in &entries {
        said.push(match count {
            1 => format!("{journal} has a new entry"),
            count => format!("{journal} has {count} new entries"),
        });
    }
    for journal in &described {
        said.push(format!("{journal} has a new description"));
    }
    if let Some(version) = record {
        said.push(format!("its record is at version {version}"));
    }
    if said.is_empty() {
        return Ok(None);
    }
    Ok(Some(format!(
        "=== the town {town} changed since you last heard: {}; its map: fastctx project map ===",
        said.join("; ")
    )))
}

fn parse<T: serde::de::DeserializeOwned>(bytes: &[u8], path: &std::path::Path) -> io::Result<T> {
    serde_json::from_slice(bytes)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, format!("{}: {error}", path.display())))
}
