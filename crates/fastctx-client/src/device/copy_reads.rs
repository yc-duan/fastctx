//! What a device reads from its replica rather than ask the machine a
//! conversation lives on, or the hub (design §28.39): the same reads a
//! machine makes of another's conversation from its copy
//! (`crate::timeline_copy`, `crate::records_copy`). A conversation's
//! timeline is read from the copy once the copy holds it whole for one
//! logged in here, a page beyond what is held taken in from the hub first;
//! its plan from the copy while the hub is not reached. A conversation the
//! hub said this device reads no more is not asked of its machine instead.

use super::Device;
use super::replica::{OlderTaken, Replica};
use crate::timeline_copy::{self, Copy};
use crate::{CallError, Error};
use fastctx_content::v2::{ConversationPlan, HeldScope, Kind as _, Timeline};
use fastctx_net::ids::HubId;
use fastctx_types::access::Written;
use fastctx_types::ipc::Failure;
use fastctx_types::timeline_pages::{TimelineAsk, TimelineTold};
use fastctx_types::{AccountName, MachineName, ResidentId};
use std::sync::Arc;

/// An item's order key and identity.
type Key = (Vec<u8>, Vec<u8>);

/// How many older pages a reading takes in at most before it answers.
const OLDER_TURNS: usize = 4;
/// How many records an older page asked of the hub holds.
const OLDER_LIMIT: u32 = 200;

fn failure(error: impl std::fmt::Display) -> Failure {
    Failure::new(error.to_string())
}

fn failed(failure: Failure) -> Error {
    Error::Call(CallError::Failed(failure))
}

/// A conversation's copy in the device's replica, for one logged in here.
pub(crate) struct Found {
    replica: Arc<Replica>,
    copy: Copy,
    hub: HubId,
    /// Those logged in here on the copy's network.
    logins: Vec<AccountName>,
}

/// What the replica holds of a conversation, as a reader reads it.
pub(crate) enum Held {
    Copy(Found),
    /// None, or more than one: the machine it lives on is asked.
    Not,
    /// A hub said this device reads it no more.
    Revoked(String),
}

impl Device {
    /// The replica this device keeps, when it keeps one (`replicate`).
    pub fn replica(&self) -> Option<Arc<Replica>> {
        self.replica.get().cloned()
    }

    /// The copy of the conversation `resident` — the one on `machine`,
    /// where named — the replica holds whole for one logged in here, its
    /// timeline made: none when none is, or two are (ids are unique on their
    /// machine alone).
    pub(crate) fn conversation_copy(&self, resident: &ResidentId, machine: Option<&Written>) -> Result<Held, Failure> {
        let Some(replica) = self.replica() else { return Ok(Held::Not) };
        let library = replica.library().clone();
        let sessions = self.sessions();
        let tail = format!("/{resident}");
        let scopes = library.scopes_ending(&tail).map_err(failure)?;
        let mut found: Option<(HeldScope, HubId, Vec<AccountName>)> = None;
        for scope in &scopes {
            if !timeline_copy::conversation_copy(scope) {
                continue;
            }
            let Some(session) = sessions.iter().find(|session| Some(session.link.membership().id.as_str()) == scope.hub.as_deref())
            else {
                continue;
            };
            let view = session.link.view();
            let logins: Vec<AccountName> = view.me().map(|me| me.logins.iter().cloned().collect()).unwrap_or_default();
            if !timeline_copy::read_by(&view.directory, scope, &logins, None) {
                continue;
            }
            // Asked of a machine: the copy of the conversation that lives
            // there or was born there, of the network that names it.
            if let Some(machine) = machine {
                if !timeline_copy::lives_on(&library, scope, &machine.name) {
                    continue;
                }
                let of_network = match &machine.hub {
                    Some(hub) => session.link.hub_name() == *hub,
                    None => {
                        let naming: Vec<_> = sessions.iter().filter(|other| other.link.view().member(&machine.name).is_some()).collect();
                        match naming.as_slice() {
                            [] => true,
                            [only] => Arc::ptr_eq(only, session),
                            _ => false,
                        }
                    }
                };
                if !of_network {
                    continue;
                }
            }
            let Some(at) = library.view_revision(&scope.key, Timeline::STREAM).map_err(failure)? else { continue };
            if at & ((1 << 40) - 1) == 0 {
                continue;
            }
            if found.replace((scope.clone(), session.link.membership().id.clone(), logins)).is_some() {
                return Ok(Held::Not);
            }
        }
        if let Some((scope, hub, logins)) = found {
            let copy = Copy::at(library, scope.key);
            return Ok(Held::Copy(Found { replica, copy, hub, logins }));
        }
        // Of none live, one a hub took from this device.
        let live: Vec<&str> = scopes.iter().map(|scope| scope.key.as_str()).collect();
        let revoked = library.keys_ending(&tail).map_err(failure)?.into_iter().find(|key| !live.contains(&key.as_str()) && replica.revoked(key));
        Ok(revoked.map_or(Held::Not, Held::Revoked))
    }
}

/// A reading of a conversation's timeline, as a device with a replica makes
/// it: from the copy where it holds the conversation, otherwise of the
/// machine it lives on (`remote`), places of the two sources kept apart.
pub(crate) async fn timeline<F, Fut>(device: &Device, machine: Option<Written>, request: TimelineAsk, remote: F) -> Result<TimelineTold, Error>
where
    F: FnOnce(Option<Written>, TimelineAsk) -> Fut,
    Fut: std::future::Future<Output = Result<TimelineTold, Error>>,
{
    let marked = timeline_copy::after(&request).is_some_and(|after| after & timeline_copy::ELSEWHERE != 0);
    if !matches!(request, TimelineAsk::Running { .. }) {
        let resident = request.resident().clone();
        match device.conversation_copy(&resident, machine.as_ref()).map_err(failed)? {
            Held::Copy(found) if marked => {
                drop(found);
                return Ok(timeline_copy::anew(&request));
            }
            Held::Copy(found) => return from_copy(&found, request).await.map_err(failed),
            Held::Revoked(scope) => return Err(failed(Failure::not_run(format!("this device reads {scope} no more")))),
            Held::Not => {}
        }
    }
    // Asked of the machine it lives on, a place read here means nothing
    // there.
    if timeline_copy::after(&request).is_some_and(|after| after != 0 && !marked) {
        return Ok(timeline_copy::anew(&request));
    }
    Ok(timeline_copy::marked(remote(machine, timeline_copy::unmarked(request)).await?))
}

/// Answers `request` from the copy. A page that reaches below what the
/// copy holds of the timeline — a window of its newest — takes the page
/// below in from the hub first: answered short, it would read as the
/// timeline's beginning. Not taken in, the reading fails.
async fn from_copy(found: &Found, request: TimelineAsk) -> Result<TimelineTold, Failure> {
    if !matches!(request, TimelineAsk::Page { .. }) {
        return timeline_copy::answer(&found.copy, request).await;
    }
    let mut turns = 0;
    loop {
        let told = timeline_copy::answer(&found.copy, request.clone()).await?;
        let reaches_below = matches!(&told, TimelineTold::Page { older: None, revision: Some(_), .. });
        let Some(below) = (if reaches_below { uncovered(found)? } else { None }) else { return Ok(told) };
        if turns == OLDER_TURNS {
            return Ok(told);
        }
        turns += 1;
        match found.replica.older(&found.hub, found.copy.scope(), Timeline::STREAM, below, OLDER_LIMIT).await {
            Ok(OlderTaken::Taken | OlderTaken::Stale | OlderTaken::Before) => {}
            Err(why) => {
                return Err(Failure::not_run(format!(
                    "the older part of this conversation is not on this device, and the hub is not reached: {why}"
                )));
            }
        }
    }
}

/// Where what the copy holds of the timeline from its top down ends, the
/// page below it to be taken in (its order key and identity; none from the
/// top); none when it holds the timeline from its first record — by its
/// frames from the first, or by pages reaching below everything.
fn uncovered(found: &Found) -> Result<Option<Option<Key>>, Failure> {
    let runs = found.copy.library().coverage(found.copy.scope(), Timeline::STREAM).map_err(failure)?;
    if runs.is_empty() {
        return Ok(None);
    }
    let Some(mut low) = runs.iter().find(|run| run.high.is_none()).map(|run| run.low.clone()) else {
        return Ok(Some(None));
    };
    loop {
        if low.0.is_empty() && low.1.is_empty() {
            return Ok(None);
        }
        match runs.iter().find(|run| run.high.as_ref() == Some(&low)) {
            Some(run) => low = run.low.clone(),
            None => return Ok(Some(Some(low))),
        }
    }
}

/// The plan of the conversation `owner` — on `machine` of the hub `hub`,
/// where named — as the replica holds it, shown to `account` (or the one
/// logged in on that network) as the hub shows it; none when the replica
/// holds no such copy for one logged in here.
pub(crate) fn plan(
    device: &Device,
    hub: Option<&fastctx_types::HubName>,
    account: Option<&AccountName>,
    owner: &ResidentId,
    machine: Option<&MachineName>,
) -> Option<fastctx_types::plan::Page> {
    let written = machine.map(|name| Written { name: name.clone(), hub: hub.cloned() });
    let Ok(Held::Copy(found)) = device.conversation_copy(owner, written.as_ref()) else { return None };
    let session = device.sessions().into_iter().find(|session| session.link.membership().id == found.hub)?;
    if hub.is_some_and(|hub| session.link.hub_name() != *hub) {
        return None;
    }
    let account = match account {
        Some(account) if found.logins.contains(account) => account.clone(),
        Some(_) => return None,
        None => match found.logins.as_slice() {
            [only] => only.clone(),
            _ => return None,
        },
    };
    let record = crate::records_copy::record_in(found.copy.library(), found.copy.scope(), ConversationPlan::STREAM, "plan")?;
    Some(crate::records_copy::plan_as_told(&session.link.view().directory, &account, owner, record))
}
