//! What a hub asks of this machine as an assistance its giver ended for good
//! is purged (`fastctx_net::wire::Down::Purge`, design-forget-account-v2.md
//! F1/F3 v2).
//!
//! Closing it here comes first, and is kept for good before anything else:
//! from then nothing is born, first started or forked here working by it
//! (`permit`). What is being born by it as it closes is waited for: each
//! birth holds a permit of the grants it is born by from before its record
//! is written until its host is asked and has answered. Only then is what
//! was born here by it told. The hub decides what goes, and asks for each;
//! what deleting each came to is kept here, so it is told again as it was
//! after the node starts again, never guessed from what is left.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fastctx_net::wire::{BornUnder, PurgeResult, PurgeStep, Purged, Purging};
use fastctx_types::access::{GrantId, HubId};
use fastctx_types::ipc::Failure;
use fastctx_types::residents::{About, BornAs, Origin};
use fastctx_types::{AccountName, ResidentId};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::Node;

/// What is kept of purges here, in the World.
const FILE: &str = "purges.json";

/// How long closing waits for what is being born by an assistance.
const BIRTHS_SETTLE: Duration = Duration::from_secs(60);

/// The purges this machine was asked, and the births held meanwhile.
#[derive(Default)]
pub(crate) struct Gate {
    inner: Arc<Inner>,
}

#[derive(Default)]
struct Inner {
    state: Mutex<State>,
    settled: tokio::sync::Notify,
}

#[derive(Default)]
struct State {
    /// What is kept, read once.
    kept: Option<Kept>,
    /// The births under way, by each grant they are born by.
    births: HashMap<GrantId, usize>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Kept {
    #[serde(default)]
    closed: Vec<Closed>,
    #[serde(default)]
    deletions: Vec<Deletion>,
}

/// An assistance closed here for good.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Closed {
    hub: HubId,
    id: u64,
    grants: Vec<GrantId>,
    account: AccountName,
    generation: Timestamp,
    at: Timestamp,
}

/// Deleting one conversation for a purge here, and what it came to.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Deletion {
    hub: HubId,
    id: u64,
    resident: ResidentId,
    state: Deleting,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "state")]
enum Deleting {
    /// Begun: a node stopped from here finishes it as any deletion
    /// (`data::delete::INTENT`), and it is told deleted.
    Begun,
    Done {
        result: PurgeResult,
    },
}

/// A birth under way, holding off the closing of the grants it is born by.
pub(crate) struct Permit {
    inner: Arc<Inner>,
    grants: Vec<GrantId>,
}

impl Drop for Permit {
    fn drop(&mut self) {
        if self.grants.is_empty() {
            return;
        }
        let mut state = self.inner.state.lock().expect("never poisoned");
        for grant in &self.grants {
            if let Some(count) = state.births.get_mut(grant) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    state.births.remove(grant);
                }
            }
        }
        drop(state);
        self.inner.settled.notify_waiters();
    }
}

fn grants(born_as: &[BornAs]) -> Vec<GrantId> {
    born_as.iter().filter_map(|born| born.grant.clone()).collect()
}

impl State {
    fn kept(&mut self, world: &Path) -> Result<&mut Kept, Failure> {
        if self.kept.is_none() {
            let path = world.join(FILE);
            let kept = match fastctx_platform::disk::read(&path) {
                Ok(bytes) => serde_json::from_slice(&bytes)
                    .map_err(|error| Failure::new(format!("reading {}: {error}", path.display())))?,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Kept::default(),
                Err(error) => return Err(Failure::new(format!("reading {}: {error}", path.display()))),
            };
            self.kept = Some(kept);
        }
        Ok(self.kept.as_mut().expect("read above"))
    }

    /// Writes `kept` for good, then holds it.
    fn keep(&mut self, world: &Path, kept: Kept) -> Result<(), Failure> {
        let bytes = serde_json::to_vec(&kept).map_err(|error| Failure::new(error.to_string()))?;
        fastctx_platform::publish(&world.join(FILE), &bytes, fastctx_platform::Permissions::Private)
            .map_err(|error| Failure::new(format!("keeping what is purged here: {error}")))?;
        self.kept = Some(kept);
        Ok(())
    }
}

/// A birth here by the rights `born_as`, to be held from before its record
/// is written until its host was asked and answered: refused where any
/// assistance it would work by was closed here for good. One born by no
/// assistance holds nothing. Where what is closed here cannot be read, no
/// birth by an assistance is let through.
pub(crate) fn permit(node: &Node, born_as: &[BornAs]) -> Result<Permit, Failure> {
    permit_in(&node.purges, node.world.path(), born_as)
}

fn permit_in(gate: &Gate, world: &Path, born_as: &[BornAs]) -> Result<Permit, Failure> {
    let inner = gate.inner.clone();
    let grants = grants(born_as);
    if grants.is_empty() {
        return Ok(Permit { inner, grants });
    }
    let mut state = inner.state.lock().expect("never poisoned");
    let kept = state.kept(world)?;
    if kept.closed.iter().any(|closed| closed.grants.iter().any(|grant| grants.contains(grant))) {
        return Err(Failure::refused(
            "the assistance it would work by was ended for good, and what was born by it is being deleted",
        ));
    }
    for grant in &grants {
        *state.births.entry(grant.clone()).or_default() += 1;
    }
    drop(state);
    Ok(Permit { inner, grants })
}

/// Whether any assistance `born_as` was born by is closed here for good;
/// where that cannot be read, it is taken to be.
pub(crate) fn closed(node: &Node, born_as: &[BornAs]) -> bool {
    let grants = grants(born_as);
    if grants.is_empty() {
        return false;
    }
    let mut state = node.purges.inner.state.lock().expect("never poisoned");
    match state.kept(node.world.path()) {
        Ok(kept) => kept.closed.iter().any(|closed| closed.grants.iter().any(|grant| grants.contains(grant))),
        Err(_) => true,
    }
}

/// Whether `about` was born here by one of the purge's assistances, for its
/// account as it was then.
fn born_under(about: &About, hub: &HubId, purging: &Purging) -> bool {
    about.born_as.as_deref().unwrap_or_default().iter().any(|born| {
        born.hub == *hub
            && born.account == purging.account
            && born.generation == purging.generation
            && born.grant.as_ref().is_some_and(|grant| purging.grants.contains(grant))
    })
}

/// What would keep `about` whole: a colleague's, or working for another
/// account as well.
fn keeps(about: &About, hub: &HubId, purging: &Purging) -> (bool, bool) {
    let colleague = matches!(about.origin, Origin::Colleague { .. }) || about.callee.is_some();
    let others = about
        .born_as
        .as_deref()
        .unwrap_or_default()
        .iter()
        .any(|born| !(born.hub == *hub && born.account == purging.account));
    (colleague, others)
}

/// Does what the hub `hub` asks of a purge here.
pub(crate) async fn told(node: &Arc<Node>, hub: &HubId, purging: Purging) -> Purged {
    let outcome = match &purging.step {
        PurgeStep::Close => close(node, hub, &purging).await,
        PurgeStep::Delete { residents } => delete(node, hub, &purging, residents).await,
    };
    outcome.unwrap_or_else(|failure| Purged::Failed { reason: failure.message })
}

/// Closes the purge's assistances here for good, kept before anything else;
/// whether it was not closed before.
fn mark_closed(gate: &Gate, world: &Path, hub: &HubId, purging: &Purging) -> Result<bool, Failure> {
    let mut state = gate.inner.state.lock().expect("never poisoned");
    let kept = state.kept(world)?;
    if kept.closed.iter().any(|closed| closed.hub == *hub && closed.id == purging.id) {
        return Ok(false);
    }
    let mut kept = kept.clone();
    kept.closed.push(Closed {
        hub: hub.clone(),
        id: purging.id,
        grants: purging.grants.clone(),
        account: purging.account.clone(),
        generation: purging.generation,
        at: Timestamp::now(),
    });
    state.keep(world, kept)?;
    Ok(true)
}

/// Whether anything is being born here by any of `grants`.
fn births_under_way(gate: &Gate, grants: &[GrantId]) -> bool {
    let state = gate.inner.state.lock().expect("never poisoned");
    grants.iter().any(|grant| state.births.contains_key(grant))
}

async fn close(node: &Arc<Node>, hub: &HubId, purging: &Purging) -> Result<Purged, Failure> {
    let inner = node.purges.inner.clone();
    if mark_closed(&node.purges, node.world.path(), hub, purging)? {
        node.log.line(format_args!(
            "closed for good the assistance of {} the hub {hub} purges ({}): nothing more is born by it here",
            purging.account, purging.id
        ));
    }
    // What is being born by it is waited for, and told with the rest.
    let deadline = tokio::time::Instant::now() + BIRTHS_SETTLE;
    loop {
        let settled = inner.settled.notified();
        tokio::pin!(settled);
        settled.as_mut().enable();
        if !births_under_way(&node.purges, &purging.grants) {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            return Ok(Purged::Waiting { why: "a conversation is being born by it here".into() });
        }
        tokio::select! {
            () = settled => {}
            () = tokio::time::sleep_until(deadline) => {}
        }
    }
    let born = node
        .residents
        .everyone()
        .filter(|resident| resident.removed.is_none() && born_under(&resident.about, hub, purging))
        .map(|resident| {
            let (colleague, others) = keeps(&resident.about, hub, purging);
            BornUnder { resident: resident.id, colleague, others }
        })
        .collect();
    Ok(Purged::Closed { born })
}

/// Keeps what deleting `resident` came to.
fn record(node: &Node, hub: &HubId, id: u64, resident: &ResidentId, state: Deleting) -> Result<(), Failure> {
    let inner = node.purges.inner.clone();
    let mut held = inner.state.lock().expect("never poisoned");
    let mut kept = held.kept(node.world.path())?.clone();
    kept.deletions.retain(|deletion| !(deletion.hub == *hub && deletion.id == id && deletion.resident == *resident));
    kept.deletions.push(Deletion { hub: hub.clone(), id, resident: resident.clone(), state });
    held.keep(node.world.path(), kept)
}

fn recorded(node: &Node, hub: &HubId, id: u64, resident: &ResidentId) -> Result<Option<Deleting>, Failure> {
    let mut state = node.purges.inner.state.lock().expect("never poisoned");
    Ok(state
        .kept(node.world.path())?
        .deletions
        .iter()
        .find(|deletion| deletion.hub == *hub && deletion.id == id && deletion.resident == *resident)
        .map(|deletion| deletion.state.clone()))
}

async fn delete(node: &Arc<Node>, hub: &HubId, purging: &Purging, residents: &[ResidentId]) -> Result<Purged, Failure> {
    let mut results = Vec::with_capacity(residents.len());
    // A Codex thread its forks still need goes once they went: those asked
    // with it go first.
    let mut later = Vec::new();
    for pass in 0..2 {
        let asked: Vec<ResidentId> = if pass == 0 { residents.to_vec() } else { std::mem::take(&mut later) };
        for resident in asked {
            match recorded(node, hub, purging.id, &resident)? {
                Some(Deleting::Done { result }) => {
                    results.push((resident, result));
                    continue;
                }
                // Begun before the node stopped: its deletion finished as any
                // interrupted one does, where it is no longer on record.
                Some(Deleting::Begun) if node.residents.about(&resident).is_none() => {
                    record(node, hub, purging.id, &resident, Deleting::Done { result: PurgeResult::Deleted })?;
                    results.push((resident, PurgeResult::Deleted));
                    continue;
                }
                _ => {}
            }
            let Some((about, _)) = node.residents.about(&resident) else {
                results.push((resident, PurgeResult::NotHere));
                continue;
            };
            let settled = |result: PurgeResult| Deleting::Done { result };
            if !born_under(&about, hub, purging) {
                record(node, hub, purging.id, &resident, settled(PurgeResult::NotBornUnder))?;
                results.push((resident, PurgeResult::NotBornUnder));
                continue;
            }
            let (colleague, others) = keeps(&about, hub, purging);
            if colleague || others {
                let why = if colleague { "a colleague's conversation" } else { "it works for another account as well" };
                let result = PurgeResult::Kept { why: why.into() };
                record(node, hub, purging.id, &resident, settled(result.clone()))?;
                results.push((resident, result));
                continue;
            }
            if about.host.agent.as_str() == "codex"
                && crate::forks::has_dependents(node, &resident).map_err(|error| Failure::new(error.to_string()))?
            {
                if pass == 0 {
                    later.push(resident);
                } else {
                    // Asked again: its forks may have gone meanwhile.
                    results.push((resident, PurgeResult::Busy { why: "its forks still need its original".into() }));
                }
                continue;
            }
            record(node, hub, purging.id, &resident, Deleting::Begun)?;
            let result = match crate::forget::purge_home(node, &resident).await {
                Ok(None) => PurgeResult::Deleted,
                Ok(Some(why)) => PurgeResult::Finishing { why },
                Err(why) => PurgeResult::Busy { why },
            };
            match &result {
                PurgeResult::Deleted => {
                    record(node, hub, purging.id, &resident, settled(PurgeResult::Deleted))?;
                    node.log.line(format_args!("deleted {resident}, born by an assistance ended for good"));
                }
                // Not deleted: asked again as it stands.
                PurgeResult::Busy { .. } if node.residents.about(&resident).is_some() => {
                    let mut state = node.purges.inner.state.lock().expect("never poisoned");
                    let mut kept = state.kept(node.world.path())?.clone();
                    kept.deletions.retain(|deletion| {
                        !(deletion.hub == *hub && deletion.id == purging.id && deletion.resident == resident)
                    });
                    state.keep(node.world.path(), kept)?;
                }
                _ => {}
            }
            results.push((resident, result));
        }
    }
    Ok(Purged::Deleted { results })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn born(grant: &str) -> Vec<BornAs> {
        vec![BornAs {
            hub: "w-0000000000000001".parse().expect("a hub"),
            account: "carol".parse().expect("an account"),
            generation: "2026-11-01T00:00:00Z".parse().expect("a time"),
            assisted: true,
            grant: Some(GrantId::try_from(grant.to_string()).expect("a grant")),
        }]
    }

    fn purging(grant: &str) -> Purging {
        Purging {
            id: 3,
            round: 1,
            grants: vec![GrantId::try_from(grant.to_string()).expect("a grant")],
            account: "carol".parse().expect("an account"),
            generation: "2026-11-01T00:00:00Z".parse().expect("a time"),
            step: PurgeStep::Close,
        }
    }

    #[test]
    fn a_birth_holds_off_closing_and_nothing_is_born_by_an_assistance_closed_for_good() {
        let world = tempfile::tempdir().expect("a directory");
        let hub: HubId = "w-0000000000000001".parse().expect("a hub");
        let gate = Gate::default();
        const ENDED: &str = "g-00000000000000aa";
        // Born by no assistance: holds nothing.
        let unassisted = permit_in(&gate, world.path(), &[]).expect("let through");
        assert!(!births_under_way(&gate, &purging(ENDED).grants));
        drop(unassisted);
        // A birth under way is waited for by its closing.
        let birth = permit_in(&gate, world.path(), &born(ENDED)).expect("let through");
        assert!(births_under_way(&gate, &purging(ENDED).grants));
        assert!(mark_closed(&gate, world.path(), &hub, &purging(ENDED)).expect("closed"));
        assert!(births_under_way(&gate, &purging(ENDED).grants));
        drop(birth);
        assert!(!births_under_way(&gate, &purging(ENDED).grants));
        // Closed: nothing more is born by it, here or after the node starts again.
        assert!(permit_in(&gate, world.path(), &born(ENDED)).is_err());
        let again = Gate::default();
        assert!(permit_in(&again, world.path(), &born(ENDED)).is_err());
        assert!(!mark_closed(&again, world.path(), &hub, &purging(ENDED)).expect("closed"));
        // Another assistance is not closed with it.
        assert!(permit_in(&again, world.path(), &born("g-00000000000000bb")).is_ok());
    }

    #[test]
    fn what_cannot_be_read_lets_no_birth_by_an_assistance_through() {
        let world = tempfile::tempdir().expect("a directory");
        fastctx_platform::disk::write(world.path().join(FILE), b"not json").expect("written");
        let gate = Gate::default();
        assert!(permit_in(&gate, world.path(), &born("g-00000000000000aa")).is_err());
        assert!(permit_in(&gate, world.path(), &[]).is_ok());
    }
}
