//! Conversation deletion records its sentinel cleanup before forgetting identity.
//!
//! What a deleted conversation owned is withdrawn from each account scope it
//! worked for. A scope that refuses for good (no such account, or this machine
//! may no longer act for it) has nothing left for this machine to withdraw; one
//! that cannot be reached now is asked again later, less often each time, and
//! given up a month after the deletion. The marker that keeps a late sentinel of
//! the deleted owner from running again goes once nothing is left to withdraw
//! and the conversation's tombstone itself is gone.
use crate::Node;
use fastctx_types::{
    Address, ResidentId,
    access::{Acting, HubAccount},
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, path::PathBuf, sync::Arc};

/// How long a removal is asked for before it is given up.
const GIVE_UP_MS: i64 = 30 * 24 * 60 * 60 * 1000;
/// The longest wait between two askings of a removal that keeps failing.
const MOST_WAIT_MS: i64 = 60 * 60 * 1000;
/// The most markers let go of in one pass.
const MARKERS_AT_ONCE: usize = 64;

#[derive(Clone, Serialize, Deserialize)]
struct Removal {
    owners: Vec<Address>,
    pending: BTreeSet<HubAccount>,
    /// When the deletion was recorded, in milliseconds; absent in removals
    /// recorded by earlier releases until first looked at.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    since: Option<i64>,
    /// How many passes in a row left something pending.
    #[serde(default, skip_serializing_if = "is_zero")]
    failures: u32,
    /// Not asked again before this time, in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    next: Option<i64>,
}

fn is_zero(value: &u32) -> bool {
    *value == 0
}

fn removals(node: &Node) -> PathBuf {
    node.world.path().join("sentinel-owner-removals")
}

fn markers(node: &Node) -> PathBuf {
    node.world.path().join("sentinel-deleted-owners")
}

pub(crate) fn note_deleted(node: &Node, id: &ResidentId) -> Result<(), String> {
    let acting = crate::here::acting(node, id).ok_or("the deleted conversation's account identity is unavailable")?;
    let mut pending: BTreeSet<_> = match &acting {
        Acting::Accounts { accounts } => accounts.iter().cloned().collect(),
        Acting::Here => BTreeSet::new(),
    };
    for link in node.links() {
        for account in node.accounts_on(&link, &acting) {
            pending.insert(HubAccount { hub: link.membership().id.clone(), account });
        }
    }
    let names = node.machine_names();
    let owners = std::iter::once(names.name)
        .chain(names.was)
        .map(|machine| Address { machine, resident: id.clone() })
        .collect::<Vec<_>>();
    fastctx_agent::state::save(&markers(node).join(format!("{id}.json")), &owners)?;
    let since = Some(jiff::Timestamp::now().as_millisecond());
    fastctx_agent::state::save(
        &removals(node).join(format!("{id}.json")),
        &Removal { owners, pending, since, failures: 0, next: None },
    )
}

/// One pass over the removals still pending and the markers left over; says
/// what could not be done, each said once by the caller while it stays so.
pub(crate) async fn recover(node: &Arc<Node>) -> Vec<String> {
    let mut errors = Vec::new();
    let directory = removals(node);
    let entries = match fastctx_platform::task::blocking(move || {
        fastctx_platform::disk::read_dir(directory).and_then(|entries| entries.collect::<std::io::Result<Vec<_>>>())
    })
    .await
    {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => {
            return vec![format!("reading sentinel owner removals: {e}")];
        }
    };
    for entry in entries {
        if let Err(error) = remove(node, entry.path(), &mut errors).await {
            errors.push(format!("sentinel owner removal remains pending: {error}"));
        }
    }
    if let Err(error) = let_markers_go(node).await {
        errors.push(format!("letting go of deleted sentinel owners' markers: {error}"));
    }
    errors
}

/// Asks each scope still pending of the removal at `path` to withdraw what
/// its owners had, each scope on its own.
async fn remove(node: &Arc<Node>, path: PathBuf, errors: &mut Vec<String>) -> Result<(), String> {
    let loaded: Option<Removal> = {
        let path = path.clone();
        fastctx_platform::task::blocking(move || fastctx_agent::state::load(&path)).await?
    };
    let Some(mut removal) = loaded else { return Ok(()) };
    let now = jiff::Timestamp::now().as_millisecond();
    let since = *removal.since.get_or_insert(now);
    let name = path.file_stem().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    if now.saturating_sub(since) > GIVE_UP_MS {
        node.log.line(format_args!(
            "giving up withdrawing the sentinels of {name}, deleted a month ago, from {}",
            scopes(&removal.pending)
        ));
        return fastctx_platform::task::blocking(move || fastctx_platform::disk::remove_file(path))
            .await
            .map_err(|e| e.to_string());
    }
    if removal.next.is_some_and(|next| next > now) {
        return Ok(());
    }
    let records = node.sentinels.records().map_err(|e| e.message)?;
    let mut left = Vec::new();
    for scope in removal.pending.clone() {
        let mut outcome = Ok(());
        for owner in &removal.owners {
            match fastctx_sentinel::lifecycle::delete_owner(
                records,
                &super::transport::Transport(node.clone()),
                &scope,
                owner,
            )
            .await
            {
                Ok(unconfirmed) => {
                    for reason in unconfirmed {
                        errors.push(format!("sentinel definition withdrawn; home has not confirmed: {reason}"));
                    }
                }
                Err(error) => {
                    outcome = Err(error.to_string());
                    break;
                }
            }
        }
        match outcome {
            Ok(()) => {
                removal.pending.remove(&scope);
            }
            Err(error) if crate::obligations::out_of_reach(&error) => {
                node.log.line(format_args!(
                    "the sentinels of {name} at {}: {error}; nothing of them is this machine's to withdraw there",
                    scope.account
                ));
                removal.pending.remove(&scope);
            }
            Err(error) => left.push(format!("{}: {error}", scope.account)),
        }
    }
    if removal.pending.is_empty() {
        return fastctx_platform::task::blocking(move || fastctx_platform::disk::remove_file(path))
            .await
            .map_err(|e| e.to_string());
    }
    removal.failures = removal.failures.saturating_add(1);
    let wait = 30_000i64.saturating_mul(1i64 << removal.failures.min(16)).min(MOST_WAIT_MS);
    removal.next = Some(now.saturating_add(wait));
    fastctx_platform::task::blocking(move || fastctx_agent::state::save(&path, &removal)).await?;
    Err(format!("{name}: {}", left.join("; ")))
}

fn scopes(pending: &BTreeSet<HubAccount>) -> String {
    pending.iter().map(|scope| scope.account.to_string()).collect::<Vec<_>>().join(", ")
}

/// Lets go of the markers of deleted owners with nothing left to withdraw
/// and no tombstone left, a bounded number at a time.
async fn let_markers_go(node: &Arc<Node>) -> std::io::Result<()> {
    let (markers, removals, tombstones) = (markers(node), removals(node), node.world.deleted());
    fastctx_platform::task::blocking(move || {
        let entries = match fastctx_platform::disk::read_dir(&markers) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        let mut gone = 0;
        for entry in entries {
            if gone >= MARKERS_AT_ONCE {
                break;
            }
            let name = entry?.file_name();
            if fastctx_platform::disk::exists(removals.join(&name))?
                || fastctx_platform::disk::exists(tombstones.join(&name))?
            {
                continue;
            }
            match fastctx_platform::disk::remove_file(markers.join(&name)) {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error),
                _ => gone += 1,
            }
        }
        Ok(())
    })
    .await
}
