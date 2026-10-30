//! How long what the composer records for its operations is kept
//! (`world-concurrency.md`, 节点上存的东西). A receipt only answers an
//! operation a device sends again, and a device sends one again only while
//! its answer is on its way: a day is ample. An edit sent again after its
//! receipt went is known by the characters it inserted (`Document::edit`), a
//! send by its epoch, so neither is done twice; an edit of an epoch since
//! sent, from a device away longer than the day, by what that epoch took,
//! kept a month (`took`), so what was said is not put back in the input
//! (review 2.1). The turns whose end the composer has seen are kept a week.
//! Rows past their time go in batches; the files an older node kept them in
//! migrate without expiring unproven legacy operation identities.

use std::sync::Arc;
use std::time::Duration;

use fastctx_storage::rusqlite::params;

use crate::Node;

/// How long an operation's receipt, or a finished upload's record, is kept.
pub(super) const RECEIPT: Duration = Duration::from_secs(24 * 60 * 60);
/// How long the end of a turn the composer saw is kept.
pub(super) const ENDED: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// How long what a sent epoch of the draft took is kept: a device away a
/// month finds its edits of it known.
pub(super) const TAKEN: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// How often rows past their time are taken away.
const PRUNING: Duration = Duration::from_secs(10 * 60);
/// How often the files of an older node are looked at again.
const SWEEPING: Duration = Duration::from_secs(6 * 60 * 60);
/// Rows taken away in one write, so a long backlog never holds the writer.
const BATCH: i64 = 2048;

/// How long a record of `kind` is kept.
pub(super) fn keep(kind: &str) -> Duration {
    match kind {
        "ended" => ENDED,
        "taken" => TAKEN,
        _ => RECEIPT,
    }
}

/// Where what the draft's epoch `epoch` took is kept.
pub(super) fn taken_key(document: &str, epoch: u64) -> String {
    format!("{document}:{epoch}")
}

/// Keeps what the draft's current epoch took, as it is sent.
pub(super) fn took(state: &mut super::state::Stored) {
    let operations = state.draft.inserting();
    if !operations.is_empty() {
        state.taken.insert(taken_key(&state.draft.id, state.draft.epoch), operations);
    }
}

/// When a record of `kind` written now expires, in milliseconds.
pub(super) fn expires(kind: &str) -> i64 {
    let keep = i64::try_from(keep(kind).as_millis()).unwrap_or(i64::MAX);
    jiff::Timestamp::now().as_millisecond().saturating_add(keep)
}

/// Takes away rows past their time, and the files of an older node past
/// theirs, for as long as the node runs.
pub(crate) fn start(node: &Arc<Node>) {
    let node = node.clone();
    tokio::spawn(async move {
        let mut swept = None::<tokio::time::Instant>;
        loop {
            tokio::time::sleep(Duration::from_secs(60)).await;
            if let Err(error) = prune(&node).await {
                node.log.line(format_args!("taking away the composer's old receipts: {error}"));
            }
            if swept.is_none_or(|at| at.elapsed() >= SWEEPING) {
                swept = Some(tokio::time::Instant::now());
                let sweeping = node.clone();
                match tokio::task::spawn_blocking(move || sweep(&sweeping)).await {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => {
                        node.log.line(format_args!("migrating an older node's composer receipts: {error}"))
                    }
                    Err(error) => node.log.line(format_args!("migrating an older node's composer receipts: {error}")),
                }
            }
            tokio::time::sleep(PRUNING.saturating_sub(Duration::from_secs(60))).await;
        }
    });
}

/// Takes away the rows past their time, a batch at a time.
async fn prune(node: &Node) -> fastctx_storage::Result<()> {
    loop {
        let now = jiff::Timestamp::now().as_millisecond();
        let taken = node
            .database
            .write(64, move |transaction| {
                Ok(transaction.execute(
                    "DELETE FROM composer_records WHERE (owner,kind,name) IN \
                     (SELECT owner,kind,name FROM composer_records WHERE expires<=?1 ORDER BY expires LIMIT ?2)",
                    params![now, BATCH],
                )?)
            })
            .await?;
        if i64::try_from(taken).unwrap_or(i64::MAX) < BATCH {
            return Ok(());
        }
    }
}

/// Imports the legacy books after their parent draft has been migrated.
fn sweep(node: &Node) -> std::io::Result<()> {
    let mut count = 0;
    let mut failures = Vec::new();
    for resident in node.residents.everyone() {
        if resident.removed.is_some() {
            continue;
        }
        let directory = node.world.conversation(&resident.id);
        match super::legacy_records::migrate(&node.database, resident.id.as_str(), &directory) {
            Ok(moved) => count += moved,
            Err(error) => failures.push(format!("{}: {error}", resident.id)),
        }
    }
    if count > 0 {
        node.log.line(format_args!("moved {count} legacy composer receipts into the database"));
    }
    if failures.is_empty() { Ok(()) } else { Err(std::io::Error::other(failures.join("; "))) }
}
