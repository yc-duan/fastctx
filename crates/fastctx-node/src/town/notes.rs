//! What a resident is told of where it lives, in the head of the answers of
//! the World's tools (`world-projects.md`, `world-map.md`): who and where it
//! is, with its project's record, the first time; and its record again
//! whenever it changed since it last heard, which replaces the one it was
//! given.

use std::io;

use fastctx_types::project::{Record, Writer};
use fastctx_types::{Address, ResidentId, TownName};

use crate::Node;
use crate::residents::Heard;

/// The lines for the head of the resident `id`'s next answer.
pub fn notes(node: &Node, id: &ResidentId) -> io::Result<(Vec<String>, Option<Heard>)> {
    let (mut notes, mut heard) = project_notes(node, id)?;
    let Some((about, town)) = node.residents.about(id) else {
        return Ok((notes, heard));
    };
    let previous = node.residents.heard(id);
    let subagents = crate::profile::subagents::configuration(node, &about).map_err(|e| io::Error::other(e.message))?;
    let subagent_prompt = subagents.as_ref().map(|v| v.instructions.as_str()).unwrap_or("");
    let subagent_hash = crate::profile::session::hash(subagent_prompt);
    let subagent_changed = about.host.agent.as_str() == "claude"
        && previous.as_ref().and_then(|h| h.subagent_prompt.as_ref()) != Some(&subagent_hash);
    if subagent_changed
        && (!subagent_prompt.is_empty() || previous.as_ref().is_some_and(|h| h.subagent_prompt.is_some()))
    {
        notes.insert(0, format!("{}\n{}", crate::profile::subagents::CHANGED, subagent_prompt));
    }
    let prompt = crate::profile::session::prompt(node, &about.acting);
    let hash = crate::profile::session::hash(&prompt);
    let user_changed = previous.as_ref().and_then(|h| h.user_prompt.as_ref()) != Some(&hash);
    if user_changed && (!prompt.is_empty() || previous.as_ref().is_some_and(|h| h.user_prompt.is_some())) {
        notes.insert(0, format!("{}\n{}", crate::profile::session::CHANGED, prompt));
    }
    if (subagent_changed || user_changed) && heard.is_none() {
        heard = Some(Heard {
            town: town.clone(),
            ledger: match town {
                Some(town) => crate::ledger::seq(&node.world, &town)?,
                None => 0,
            },
            user_prompt: None,
            subagent_prompt: None,
        });
    }
    if let Some(heard) = &mut heard {
        heard.user_prompt = Some(hash);
        heard.subagent_prompt = (about.host.agent.as_str() == "claude").then_some(subagent_hash);
    }
    Ok((notes, heard))
}

fn project_notes(node: &Node, id: &ResidentId) -> io::Result<(Vec<String>, Option<Heard>)> {
    let here = node.machine();
    let Some(town) = node.residents.town_of(id) else { return Ok((Vec::new(), None)) };
    let ledger = match &town {
        Some(town) => crate::ledger::seq(&node.world, town)?,
        None => 0,
    };
    let now = Heard { subagent_prompt: None, user_prompt: None, town: town.clone(), ledger };
    let notes = match node.residents.heard(id) {
        None => vec![introduction(node, id, town.as_ref())?],
        // It moved: entering gave it the new town's record.
        Some(heard) if heard.town != town => Vec::new(),
        Some(heard) if heard.ledger >= ledger => return Ok((Vec::new(), None)),
        Some(heard) => {
            let me = Address { machine: here.clone(), resident: id.clone() };
            match &town {
                Some(town) => news(node, id, town, heard.ledger, &me)?.into_iter().collect(),
                None => Vec::new(),
            }
        }
    };
    Ok((notes, Some(now)))
}

fn introduction(node: &Node, id: &ResidentId, town: Option<&TownName>) -> io::Result<String> {
    Ok(match town {
        Some(town) => match super::instructions::text(node, town, &super::instructions::acting(node, id))? {
            Some(record) => format!("=== you are {id}, working in the project {town} ===\n{record}"),
            None => format!("=== you are {id}, working in the project {town}; its record: `fastctx project show` ==="),
        },
        // When to enter a project is said once, in the session's guidance.
        None => format!("=== you are {id}, in no project ==="),
    })
}

/// The town's record, when it changed after `after` other than by `me`;
/// `None` when it did not. It replaces the record the resident was given,
/// with the user's instructions in it. Journals are project memory, which
/// is not wired in until it is finished (`world-journals.md`), so their
/// news is not told.
fn news(node: &Node, id: &ResidentId, town: &TownName, after: u64, me: &Address) -> io::Result<Option<String>> {
    let world = &node.world;
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
    Ok(Some(match super::instructions::text(node, town, &super::instructions::acting(node, id))? {
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
