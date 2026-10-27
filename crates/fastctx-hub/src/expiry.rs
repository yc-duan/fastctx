//! Letters kept for a machine that has been away too long are settled as
//! not to be done (`world-network.md`, 每一笔账都有终点): a machine offline
//! more than `AWAY` gets none of what waited for it, and what waited no
//! longer grows on the hub's disk. A machine taken off the hub has its
//! letters set aside at once (`crate::store::Store::forget_letters`).

use std::sync::{Arc, Weak};
use std::time::Duration;

use fastctx_types::MachineName;
use jiff::Timestamp;

use crate::Hub;

/// How long a machine may be away before what waits for it is settled.
pub(crate) const AWAY: Duration = Duration::from_secs(30 * 24 * 60 * 60);
/// How often the hub looks for machines away that long.
const LOOKING: Duration = Duration::from_secs(6 * 60 * 60);

/// Settles the letters of machines away too long, for as long as the hub runs.
pub(crate) async fn settle(hub: Weak<Hub>) {
    loop {
        tokio::time::sleep(Duration::from_secs(10 * 60)).await;
        let Some(hub) = hub.upgrade() else { return };
        for name in away(&hub) {
            match settle_one(&hub, &name).await {
                Ok(0) => {}
                Ok(settled) => hub.log.line(format_args!(
                    "{settled} letters for {name}, away more than 30 days, were settled as not delivered"
                )),
                Err(error) => hub.log.line(format_args!("settling the letters for {name}: {error}")),
            }
        }
        drop(hub);
        tokio::time::sleep(LOOKING.saturating_sub(Duration::from_secs(10 * 60))).await;
    }
}

/// The machines not connected that were last seen more than `AWAY` ago.
/// One whose leaving is not known (the hub stopped while it was connected)
/// is not counted away.
fn away(hub: &Hub) -> Vec<MachineName> {
    let Ok(away) = jiff::SignedDuration::try_from(AWAY) else { return Vec::new() };
    let Ok(before) = Timestamp::now().checked_sub(away) else { return Vec::new() };
    let store = hub.store();
    store
        .letters
        .iter()
        .filter(|(_, letters)| letters.connected_at.is_none() && letters.last_seen.is_some_and(|seen| seen < before))
        .filter(|(name, _)| store.outbox_of(name).is_some())
        .map(|(name, _)| name.clone())
        .collect()
}

/// Settles what waits for `name`: each letter is said in the log, by whom it
/// was sent, and taken away once the records number past it, as a letter
/// taken is (`crate::mailbox`).
async fn settle_one(hub: &Arc<Hub>, name: &MachineName) -> std::io::Result<usize> {
    let (epoch, dir, records) = {
        let store = hub.store();
        let Some((epoch, dir)) = store.outbox_of(name) else { return Ok(0) };
        (epoch, dir, store.records.clone())
    };
    let reading = dir.clone();
    let found = tokio::task::spawn_blocking(move || -> std::io::Result<Vec<(u64, String)>> {
        let mut found = Vec::new();
        for (seq, _) in crate::store::letter_numbers(&reading)? {
            let from =
                crate::store::read_letter(&reading, seq)?.map(|message| message.from.to_string()).unwrap_or_default();
            found.push((seq, from));
        }
        Ok(found)
    })
    .await
    .map_err(|error| std::io::Error::other(error.to_string()))??;
    if found.is_empty() {
        return Ok(0);
    }
    // Still away: one that came back meanwhile takes its letters.
    let revision = {
        let mut store = hub.store();
        if store.letters.get(name).is_none_or(|letters| letters.connected_at.is_some()) {
            return Ok(0);
        }
        store.save_letters()?;
        store.revision()
    };
    records.written(revision).await;
    {
        let mut store = hub.store();
        if store.letters.get(name).is_none_or(|letters| letters.connected_at.is_some()) {
            return Ok(0);
        }
        crate::mailbox::settled(&mut store, name, epoch, found.iter().map(|(seq, _)| *seq));
    }
    let mut senders: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
    for (_, from) in &found {
        *senders.entry(from.as_str()).or_default() += 1;
    }
    hub.log.line(format_args!(
        "letters for {name} settled as not delivered, by sender: {}",
        senders.iter().map(|(from, count)| format!("{from} {count}")).collect::<Vec<_>>().join(", ")
    ));
    let settled = found.len();
    tokio::task::spawn_blocking(move || {
        for (seq, _) in found {
            match std::fs::remove_file(dir.join(format!("{seq:020}"))) {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error),
                _ => {}
            }
        }
        Ok(())
    })
    .await
    .map_err(|error| std::io::Error::other(error.to_string()))??;
    Ok(settled)
}
