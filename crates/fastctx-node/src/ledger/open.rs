//! Opening the entries this machine keeps sealed, into their towns.
//!
//! An entry opens with a copy of its key sealed for this machine, in the
//! entry itself or in a later item that seals it once more. What opens is
//! the entry its author signed, checked as every machine checks it: its
//! author a member when it wrote it, its town the one whose id it came
//! under, and the member that sealed it its author, or one with management
//! rights carrying it over from an earlier version of the network. The
//! World's own ledger holds the naming key instead (`super::names`).

use std::collections::BTreeMap;
use std::sync::Arc;

use fastctx_net::ledger::{self, Item, Share, TownId, WorldEntry};
use fastctx_net::link::Link;
use fastctx_net::sign::Domain;

use super::items::Items;
use crate::Node;

/// Opens every entry of `town` kept here and not opened yet that this
/// machine has a key for. Returns what could not be opened and why, beyond
/// waiting for a key.
pub fn town(node: &Arc<Node>, link: &Link, town: &TownId) -> Result<Vec<String>, String> {
    let store = Items::new(&node.world);
    let mut state = store.state(town).map_err(|error| format!("{town}: {error}"))?;
    if state.unopened.is_empty() {
        return Ok(Vec::new());
    }
    let items = store.all(town).map_err(|error| format!("{town}: {error}"))?;
    let mut shares: BTreeMap<u64, Vec<Share>> = BTreeMap::new();
    for (_, item, _) in &items {
        if let Item::Share(share) = item {
            shares.entry(share.entry).or_default().push(share.clone());
        }
    }
    let (table, _) = link.members();
    let world = link.membership().world.clone();
    let mut problems = Vec::new();
    for (seq, item, _) in &items {
        let Item::Entry(entry) = item else { continue };
        if !state.unopened.contains(seq) {
            continue;
        }
        let key = match link.entry_key(entry, shares.get(seq).into_iter().flatten()) {
            Ok(Some(key)) => key,
            Ok(None) => continue,
            Err(reason) => {
                refuse(&mut state, *seq, format!("entry {seq} of {town}: {reason}"), &mut problems);
                continue;
            }
        };
        let inner = match ledger::sealed::open_entry(entry, &key) {
            Ok(inner) => inner,
            Err(reason) => {
                refuse(&mut state, *seq, format!("entry {seq} of {town}: {reason}"), &mut problems);
                continue;
            }
        };
        if *town == TownId::world() {
            match world_entry(&inner, &table, &entry.author) {
                Ok(naming) => {
                    super::names::set(node, naming);
                    state.unopened.remove(seq);
                }
                Err(reason) => refuse(&mut state, *seq, format!("the World's own ledger: {reason}"), &mut problems),
            }
            continue;
        }
        // Whether the entry is of the town it came under is known by its
        // name and the naming key; without the key yet, it waits.
        let Some(naming) = node.naming.read().expect("never poisoned").clone() else { continue };
        let opened = ledger::open(&inner, &world, &table).and_then(|opened| {
            if ledger::town_id(&naming, &opened.town) != *town {
                return Err(format!("it is of the town {}, not the one it came under", opened.town));
            }
            if opened.author != entry.author && !table.get(&entry.author).is_some_and(|member| member.manage) {
                return Err(format!("{} sealed an entry {} wrote", entry.author, opened.author));
            }
            Ok(opened)
        });
        match opened {
            Ok(opened) => match super::apply(&node.world, *seq, &opened, &inner) {
                Ok(()) => {
                    state.unopened.remove(seq);
                }
                Err(error) => problems.push(format!("taking entry {seq} of {}: {error}", opened.town)),
            },
            Err(reason) => refuse(&mut state, *seq, format!("entry {seq} of {town}: {reason}"), &mut problems),
        }
    }
    store.set_state(town, &state).map_err(|error| format!("{town}: {error}"))?;
    Ok(problems)
}

fn refuse(state: &mut super::items::State, seq: u64, reason: String, problems: &mut Vec<String>) {
    state.unopened.remove(&seq);
    state.refused.insert(seq, reason.clone());
    problems.push(reason);
}

/// The naming key the World's own ledger holds, if a member with
/// management rights wrote it.
fn world_entry(
    inner: &fastctx_net::sign::Signed,
    table: &fastctx_net::trust::Table,
    sealed_by: &fastctx_types::MachineName,
) -> Result<ledger::NamingKey, String> {
    let claimed: WorldEntry = inner.claim().map_err(|error| format!("it cannot be read: {error}"))?;
    if claimed.author != *sealed_by {
        return Err(format!("{sealed_by} sealed an entry {} wrote", claimed.author));
    }
    let key = match (table.get(&claimed.author), table.revoked(&claimed.author)) {
        (Some(member), _) => member.key,
        (None, Some(revocation)) if claimed.at < revocation.at => revocation.key,
        _ => return Err(format!("{} is not a member", claimed.author)),
    };
    let opened: WorldEntry = inner.open(&key, Domain::Ledger).map_err(|error| error.to_string())?;
    Ok(opened.naming)
}
