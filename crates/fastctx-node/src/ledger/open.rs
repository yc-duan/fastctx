//! Opening the entries this machine keeps sealed, into their towns.
//!
//! An entry opens with a copy of its key sealed for this machine, in the
//! entry itself or in a later item that seals it once more; when none of
//! the copies here opens it, it waits for one that does. What opens is the
//! entry its author signed, checked as every machine checks it: its author
//! a machine trusted when it wrote it, whose scope then covered the town;
//! its town the one whose id it came under; and the tokens it showed the
//! hub those of the paths it writes. Who sealed it does not matter: a
//! member with management rights carries entries over from an earlier
//! version of the network, and what is in them is the author's. The
//! World's own ledger holds the naming key instead (`super::names`).

use std::collections::BTreeMap;
use std::sync::Arc;

use fastctx_net::ledger::{self, Item, Share, TownId, WorldEntry};
use fastctx_net::link::Link;
use fastctx_net::sign::{Domain, Signed};
use fastctx_net::trust::Table;

use super::items::{Items, State};
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
    let shares = shares(&items);
    let (table, _) = link.members();
    let world = link.membership().world.clone();
    let mut problems = Vec::new();
    for (seq, item, _) in &items {
        let Item::Entry(entry) = item else { continue };
        if !state.unopened.contains(seq) {
            continue;
        }
        let inner = match link.open_sealed(entry, shares.get(seq).into_iter().flatten()) {
            Ok(Some((_, inner))) => inner,
            Ok(None) => continue,
            // A copy that opens may still come; the entry waits for it.
            Err(reason) => {
                problems.push(format!("entry {seq} of {town}: {reason}"));
                continue;
            }
        };
        if *town == TownId::world() {
            if node.naming.read().expect("never poisoned").is_some() {
                refuse(&mut state, *seq, format!("the World's own ledger: entry {seq} is a second naming key"), &mut problems);
                continue;
            }
            match world_entry(&inner, &table) {
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
            let paths: Vec<String> = opened.files.iter().map(|file| file.path.clone()).collect();
            ledger::sealed::check_tokens(entry, &naming, &paths)?;
            if !table.could_write(&opened.author, town, opened.at) {
                return Err(format!("{}'s scope did not include {} when it wrote this", opened.author, opened.town));
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

/// The copies of entries' keys sealed again, by the number of the entry.
pub(super) fn shares(items: &[(u64, Item, Signed)]) -> BTreeMap<u64, Vec<Share>> {
    let mut shares: BTreeMap<u64, Vec<Share>> = BTreeMap::new();
    for (_, item, _) in items {
        if let Item::Share(share) = item {
            shares.entry(share.entry).or_default().push(share.clone());
        }
    }
    shares
}

fn refuse(state: &mut State, seq: u64, reason: String, problems: &mut Vec<String>) {
    state.unopened.remove(&seq);
    state.refused.insert(seq, reason.clone());
    problems.push(reason);
}

/// The naming key the World's own ledger holds, if a member with
/// management rights when it wrote it wrote it.
pub(super) fn world_entry(inner: &Signed, table: &Table) -> Result<ledger::NamingKey, String> {
    let claimed: WorldEntry = inner.claim().map_err(|error| format!("it cannot be read: {error}"))?;
    let key = table.author_key(&claimed.author, claimed.at)?;
    let opened: WorldEntry = inner.open(&key, Domain::Ledger).map_err(|error| error.to_string())?;
    if !table.managed_at(&opened.author, opened.at) {
        return Err(format!("{} did not hold management rights when it wrote it", opened.author));
    }
    Ok(opened.naming)
}
