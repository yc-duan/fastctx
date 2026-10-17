//! What a resident is told of where it lives, in the head of the answers of
//! the World's tools (`world-projects.md`, `world-map.md`): who and where it
//! is, with its project's record, the first time; and its record again
//! whenever it changed since it last heard, which replaces the one it was
//! given.

use std::io;

use fastctx_types::layout::WorldDir;
use fastctx_types::project::{Record, Writer};
use fastctx_types::{Address, MachineName, ResidentId, TownName};

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
    // A session that may not work on this computer is not told it is here
    // (`world-network.md`, 一个居民替谁干活).
    let works_here = node.residents.about(id).is_none_or(|(about, _)| node.works_here(&about.acting).is_ok());
    let notes = match node.residents.heard(id) {
        None => vec![introduction(&node.world, id, works_here.then_some(&here), town.as_ref())?],
        // It moved: entering gave it the new town's record.
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

fn introduction(
    world: &WorldDir,
    id: &ResidentId,
    here: Option<&MachineName>,
    town: Option<&TownName>,
) -> io::Result<String> {
    let on = here.map(|here| format!(" on {here}")).unwrap_or_default();
    Ok(match town {
        Some(town) => match super::instructions::text(world, town)? {
            Some(record) => format!("=== you are {id}{on}, working in the project {town} ===\n{record}"),
            None => {
                format!("=== you are {id}{on}, working in the project {town}; its record: `fastctx project show` ===")
            }
        },
        // When to enter a project is said once, in the session's guidance.
        None => format!("=== you are {id}{on}, in no project ==="),
    })
}

/// The town's record, when it changed after `after` other than by `me`;
/// `None` when it did not. It replaces the record the resident was given,
/// with the user's instructions in it. Journals are project memory, which
/// is not wired in until it is finished (`world-journals.md`), so their
/// news is not told.
fn news(world: &WorldDir, town: &TownName, after: u64, me: &Address) -> io::Result<Option<String>> {
    let mine = |by: &Writer| matches!(by, Writer::Resident { address } if address == me);
    let ledger = world.town(town).ledger();
    let mut record: Option<u32> = None;
    for (_, entry) in crate::ledger::entries_after(world, town, after)? {
        for file in &entry.files {
            let parts: Vec<&str> = file.path.split('/').collect();
            if let ["project", _] = parts.as_slice() {
                let version: Record = parse(&file.bytes.0, &ledger.join(&file.path))?;
                if !mine(&version.by) {
                    record = record.max(Some(version.version));
                }
            }
        }
    }
    let Some(version) = record else { return Ok(None) };
    Ok(Some(match super::instructions::text(world, town)? {
        Some(record) => format!(
            "=== the project {town} changed since you last heard; its record now, which replaces the one you were given ===\n{record}"
        ),
        None => format!(
            "=== the project {town} changed since you last heard: its record is at version {version}; read it with `fastctx project show` ==="
        ),
    }))
}

fn parse<T: serde::de::DeserializeOwned>(bytes: &[u8], path: &std::path::Path) -> io::Result<T> {
    serde_json::from_slice(bytes)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, format!("{}: {error}", path.display())))
}
