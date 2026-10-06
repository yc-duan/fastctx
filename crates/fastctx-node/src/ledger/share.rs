//! Sealing entries' keys again for the members that may read a town and
//! have no copy yet (`world-network.md`, 内容怎样加密): a member that just
//! joined, whose scope grew, or that published its sealing key late. A
//! machine with management rights does it for everyone; a member that
//! invited members of its own branch does it for them. Two machines doing
//! it at once seal two identical copies, which is no conflict.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use fastctx_net::ledger::{Item, TownId, sealed};
use fastctx_net::link::Link;
use fastctx_types::MachineName;

use super::items::Items;
use crate::Node;

/// Seals what is missing in every town kept here. Returns what failed.
pub async fn reconcile(node: &Arc<Node>, link: &Link) -> Vec<String> {
    let Some(me) = link.me() else { return Vec::new() };
    let (table, _) = link.members();
    let mine: BTreeSet<MachineName> = table
        .members()
        .filter(|member| member.delegated && member.invited_by.as_ref() == Some(&me.name))
        .map(|member| member.name.clone())
        .collect();
    if !me.manage && mine.is_empty() {
        return Vec::new();
    }
    let store = Items::new(&node.world);
    let towns = match store.towns() {
        Ok(towns) => towns,
        Err(error) => return vec![format!("reading the ledgers kept here: {error}")],
    };
    let mut problems = Vec::new();
    for town in towns {
        let readers: Vec<sealed::Reader> = sealed::readers(&table, &town)
            .into_iter()
            .filter(|reader| reader.name != me.name && (me.manage || mine.contains(&reader.name)))
            .collect();
        if readers.is_empty() {
            continue;
        }
        if let Err(error) = town_shares(link, &store, &town, &readers).await {
            problems.push(format!("sealing keys of {town} again: {error}"));
        }
    }
    problems
}

async fn town_shares(link: &Link, store: &Items, town: &TownId, readers: &[sealed::Reader]) -> Result<(), String> {
    let items = store.all(town).map_err(|error| error.to_string())?;
    let mut holders: BTreeMap<u64, BTreeSet<MachineName>> = BTreeMap::new();
    let mut shares: BTreeMap<u64, Vec<fastctx_net::ledger::Share>> = BTreeMap::new();
    for (seq, item, _) in &items {
        match item {
            Item::Entry(entry) => {
                holders.entry(*seq).or_default().extend(entry.keys.iter().map(|wrapped| wrapped.to.clone()));
            }
            Item::Share(share) => {
                holders.entry(share.entry).or_default().insert(share.to.clone());
                shares.entry(share.entry).or_default().push(share.clone());
            }
        }
    }
    for (seq, item, _) in &items {
        let Item::Entry(entry) = item else { continue };
        let held = holders.get(seq);
        let missing: Vec<&sealed::Reader> =
            readers.iter().filter(|reader| !held.is_some_and(|held| held.contains(&reader.name))).collect();
        if missing.is_empty() {
            continue;
        }
        // Only what this machine can open, it can seal again.
        let Some(key) = link.entry_key(entry, shares.get(seq).into_iter().flatten())? else { continue };
        for reader in missing {
            let share = link.share(entry, *seq, &key, reader)?;
            link.ledger_write(share).await?;
        }
    }
    Ok(())
}
