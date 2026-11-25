//! What a device reads from its replica rather than ask the machine a
//! conversation lives on, or the hub (design §28.39): the same reads a
//! machine makes of another's conversation from its copy
//! (`crate::timeline_copy`, `crate::records_copy`). A conversation's
//! timeline is read from the copy once the copy holds it whole for one
//! logged in here; a page is read within the stretch the copy holds without
//! a gap, one reaching below it taken in from the hub first, and the latest
//! page with enough beneath it held for the pages read after it. Its plan
//! is read from the copy while the hub is not reached. A conversation the
//! hub said this device reads no more is not asked of its machine instead.

use super::Device;
use super::replica::{OlderTaken, Replica};
use crate::timeline_copy::{self, Copy};
use crate::{CallError, Error};
use fastctx_content::v2::{ConversationPlan, Covered, HeldScope, Kind as _, Timeline};
use fastctx_net::ids::HubId;
use fastctx_net::link::Status;
use fastctx_types::access::Written;
use fastctx_types::ipc::Failure;
use fastctx_types::timeline_pages::{TimelineAsk, TimelineEvent, TimelineFrom, TimelineTold};
use fastctx_types::{AccountName, MachineName, ResidentId};
use std::sync::Arc;

/// An item's order key and identity.
type Key = (Vec<u8>, Vec<u8>);

/// How many older pages a reading takes in at most before it answers.
const OLDER_TURNS: usize = 6;
/// How many records an older page asked of the hub holds.
const OLDER_LIMIT: u32 = 200;
/// How many events below the latest the copy holds without a gap before the
/// latest page is answered: as many as a reader reads on from it to the
/// turn it is in (the interface's `HELD_EVENTS`), so the pages it reads
/// after are read as they are held, nothing taken in under them.
const BENEATH_LATEST: u32 = 800;

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
}

/// What the replica holds of a conversation, as a reader reads it.
pub(crate) enum Held {
    Copy(Found),
    /// None, or more than one: the machine it lives on is asked.
    Not,
    /// A hub said this device reads it no more.
    Revoked(String),
}

/// Whether the copy `key` is of the conversation on `machine`, where named,
/// by its key: `c/<hub>/<machine>/<resident>`.
fn named_by_key(key: &str, machine: Option<&Written>) -> bool {
    machine.is_none_or(|machine| key.split('/').nth(2) == Some(machine.name.as_str()))
}

impl Device {
    /// The replica this device keeps, when it keeps one (`replicate`).
    pub fn replica(&self) -> Option<Arc<Replica>> {
        self.replica.get().cloned()
    }

    /// The copies of the conversation `resident` — the one on `machine`,
    /// where named — the replica holds whole for one logged in here
    /// (`only`, where named), with the network each is of: a hub said to
    /// read no more since the replica started is gone past.
    fn copies(
        &self,
        replica: &Replica,
        resident: &ResidentId,
        machine: Option<&Written>,
        only: Option<&AccountName>,
    ) -> Result<Vec<(HeldScope, HubId)>, Failure> {
        let library = replica.library();
        let sessions = self.sessions();
        let mut copies = Vec::new();
        for scope in library.scopes_ending(&format!("/{resident}")).map_err(failure)? {
            if !timeline_copy::conversation_copy(&scope) || replica.revoked(&scope.key) {
                continue;
            }
            let Some(session) =
                sessions.iter().find(|session| Some(session.link.membership().id.as_str()) == scope.hub.as_deref())
            else {
                continue;
            };
            let view = session.link.view();
            let logins: Vec<AccountName> = view.me().map(|me| me.logins.iter().cloned().collect()).unwrap_or_default();
            if !timeline_copy::read_by(&view.directory, &scope, &logins, only) {
                continue;
            }
            // Asked of a machine: the copy of the conversation that lives
            // there or was born there, of the network that names it.
            if let Some(machine) = machine {
                if !timeline_copy::lives_on(library, &scope, &machine.name) {
                    continue;
                }
                let of_network = match &machine.hub {
                    Some(hub) => session.link.hub_name() == *hub,
                    None => {
                        let naming: Vec<_> =
                            sessions.iter().filter(|other| other.link.view().member(&machine.name).is_some()).collect();
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
            copies.push((scope, session.link.membership().id.clone()));
        }
        Ok(copies)
    }

    /// The copy of the conversation `resident` — the one on `machine`,
    /// where named — the replica holds whole for one logged in here, its
    /// timeline made: none when none is, or two are (ids are unique on their
    /// machine alone).
    pub(crate) fn conversation_copy(&self, resident: &ResidentId, machine: Option<&Written>) -> Result<Held, Failure> {
        let Some(replica) = self.replica() else { return Ok(Held::Not) };
        let library = replica.library().clone();
        let mut found = None;
        for (scope, hub) in self.copies(&replica, resident, machine, None)? {
            // With a hub that keeps no conversation's streams
            // (`fastctx_types::assets::DIRECT`), what was written of one
            // since stays on the machine that orders it, and nothing of it
            // comes to this device's copy: it is read where it lives.
            let kept_elsewhere = self.sessions().iter().any(|session| {
                session.link.membership().id == hub && session.link.asset_version() >= fastctx_types::assets::DIRECT
            });
            if kept_elsewhere {
                continue;
            }
            let Some(at) = library.view_revision(&scope.key, Timeline::STREAM).map_err(failure)? else { continue };
            if at & ((1 << 40) - 1) == 0 {
                continue;
            }
            if found.replace((scope.key, hub)).is_some() {
                return Ok(Held::Not);
            }
        }
        if let Some((key, hub)) = found {
            return Ok(Held::Copy(Found { replica, copy: Copy::at(library, key), hub }));
        }
        // Of none live, one a hub took from this device — of the machine
        // named, where it is (codex-f92d833e phone #10).
        let revoked = library
            .keys_ending(&format!("/{resident}"))
            .map_err(failure)?
            .into_iter()
            .find(|key| replica.revoked(key) && named_by_key(key, machine));
        Ok(revoked.map_or(Held::Not, Held::Revoked))
    }
}

/// A reading of a conversation's timeline, as a device with a replica makes
/// it: from the copy where it holds the conversation, otherwise of the
/// machine it lives on (`remote`), places of the two sources kept apart.
pub(crate) async fn timeline<F, Fut>(
    device: &Device,
    machine: Option<Written>,
    request: TimelineAsk,
    remote: F,
) -> Result<TimelineTold, Error>
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
            Held::Copy(found) => {
                let told = from_copy(&found, request.clone()).await.map_err(failed)?;
                // An object the copy does not hold yet — a light device's
                // larger ones come as they are read — is read where the
                // conversation lives (codex-f92d833e phone #3).
                if !matches!(told, TimelineTold::Object { base64: None }) {
                    return Ok(told);
                }
            }
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

/// Answers `request` from the copy; a page as `page` reads it.
async fn from_copy(found: &Found, request: TimelineAsk) -> Result<TimelineTold, Failure> {
    match &request {
        TimelineAsk::Page { from: TimelineFrom::Latest | TimelineFrom::Before { .. }, .. } => {
            page(found, request).await
        }
        _ => timeline_copy::answer(&found.copy, request).await,
    }
}

/// A page read down from the latest or from a place, within the stretch the
/// copy holds without a gap from where it begins (codex-f92d833e phone #4):
/// what lies below that stretch is left out, the page saying more is older,
/// so the reader asks below it next and that page is taken in from the hub
/// then. Nothing held where it begins, the page there is taken in first.
/// The latest page is answered once enough beneath it is held (#5). A page
/// that cannot be taken in — the hub not reached — fails rather than show
/// what is held as the beginning (#9).
async fn page(found: &Found, request: TimelineAsk) -> Result<TimelineTold, Failure> {
    let TimelineAsk::Page { from, .. } = &request else { return timeline_copy::answer(&found.copy, request).await };
    let latest = matches!(from, TimelineFrom::Latest);
    let begins: Option<Key> = match from {
        TimelineFrom::Before { place } => {
            let place = timeline_copy::decode_place(place.clone())?;
            Some((place.order, place.item))
        }
        _ => None,
    };
    for turn in 0..=OLDER_TURNS {
        let runs = found.copy.library().coverage(found.copy.scope(), Timeline::STREAM).map_err(failure)?;
        // Held by its frames from the first: every page is held.
        if runs.is_empty() {
            return timeline_copy::answer(&found.copy, request).await;
        }
        let Some(run) = run_of(&runs, begins.as_ref()) else {
            take_in(found, begins.clone(), latest).await?;
            continue;
        };
        let low = run.low.clone();
        let bottom = low.0.is_empty() && low.1.is_empty();
        if latest && !bottom && turn < OLDER_TURNS && held_beneath(found, &low)? < BENEATH_LATEST {
            // Enough beneath the latest is taken in before it is read; not
            // reached, what is held is read all the same, cut at its gap.
            if take_in(found, Some(low.clone()), true).await? {
                continue;
            }
        }
        let mut told = timeline_copy::answer(&found.copy, request.clone()).await?;
        let TimelineTold::Page { events, older, .. } = &mut told else { return Ok(told) };
        if bottom {
            return Ok(told);
        }
        match cut(events, &low)? {
            // The copy holds nothing below this run, but the conversation
            // goes on below it: a light copy's recent part is not where it
            // begins, so the page still says more is older.
            Cut::Whole => {
                if older.is_none() {
                    *older = events.first().map(|oldest| oldest.place.clone());
                }
                return Ok(told);
            }
            Cut::Below(oldest) => {
                *older = Some(oldest);
                return Ok(told);
            }
            Cut::Nothing => {
                if turn < OLDER_TURNS && take_in(found, Some(low), latest).await? {
                    continue;
                }
                return Err(Failure::not_run(
                    "the older part of this conversation is not on this device, and the hub is not reached".to_string(),
                ));
            }
        }
    }
    Err(Failure::not_run("the older part of this conversation could not be taken in from the hub now".to_string()))
}

/// The run of `runs` a page read down from `begins` (the latest where none)
/// begins in: the one reaching above everything, or the one holding what is
/// just below `begins`.
fn run_of<'a>(runs: &'a [Covered], begins: Option<&Key>) -> Option<&'a Covered> {
    match begins {
        None => runs.iter().find(|run| run.high.is_none()),
        Some(key) => runs.iter().find(|run| run.low < *key && run.high.as_ref().is_none_or(|high| key <= high)),
    }
}

/// How a page read down stands against the low end of the run it began in.
#[derive(Debug, PartialEq)]
enum Cut {
    /// Every event is within the run.
    Whole,
    /// Those below it are left out; more is older, from the oldest kept.
    Below(fastctx_types::timeline_pages::TimelinePlace),
    /// None is within it.
    Nothing,
}

/// Leaves out of `events` (oldest first) those below `low`.
fn cut(events: &mut Vec<TimelineEvent>, low: &Key) -> Result<Cut, Failure> {
    let before = events.len();
    let mut kept = Vec::with_capacity(events.len());
    for event in events.drain(..) {
        let place = timeline_copy::decode_place(event.place.clone())?;
        if (place.order, place.item) >= *low {
            kept.push(event);
        }
    }
    *events = kept;
    Ok(match events.first() {
        None => Cut::Nothing,
        Some(_) if events.len() == before => Cut::Whole,
        Some(oldest) => Cut::Below(oldest.place.clone()),
    })
}

/// How many events the copy holds from `low` up to the latest, up to
/// `BENEATH_LATEST`.
fn held_beneath(found: &Found, low: &Key) -> Result<u32, Failure> {
    let read = found
        .copy
        .library()
        .view_page(found.copy.scope(), Timeline::STREAM, &fastctx_content::Anchor::Latest, BENEATH_LATEST, u32::MAX)
        .map_err(failure)?;
    Ok(read.entries.iter().filter(|entry| (entry.order.clone(), entry.item.clone()) >= *low).count() as u32)
}

/// Takes in the page of the timeline below `below` (the top where none)
/// from the hub; whether it was. Not taken in is an error unless `quiet`.
async fn take_in(found: &Found, below: Option<Key>, quiet: bool) -> Result<bool, Failure> {
    match found.replica.older(&found.hub, found.copy.scope(), Timeline::STREAM, below, OLDER_LIMIT).await {
        Ok(OlderTaken::Taken | OlderTaken::Stale | OlderTaken::Before) => Ok(true),
        Err(_) if quiet => Ok(false),
        Err(why) => Err(Failure::not_run(format!(
            "the older part of this conversation is not on this device, and the hub is not reached: {why}"
        ))),
    }
}

/// The plan of the conversation `owner` — on `machine` of the hub `hub`,
/// where named — as the replica holds it, shown to `account` (or the one
/// logged in on that network) as the hub shows it: only while that hub is
/// not reached, and only a copy that account reads (codex-f92d833e phone
/// #1, #2); its timeline need not be made (#8). None when the replica holds
/// no such copy.
pub(crate) fn plan(
    device: &Device,
    hub: Option<&fastctx_types::HubName>,
    account: Option<&AccountName>,
    owner: &ResidentId,
    machine: Option<&MachineName>,
) -> Option<fastctx_types::plan::Page> {
    let replica = device.replica()?;
    let written = machine.map(|name| Written { name: name.clone(), hub: hub.cloned() });
    let sessions = device.sessions();
    let session = |id: &HubId| sessions.iter().find(|session| session.link.membership().id == *id).cloned();
    let candidates = device.copies(&replica, owner, written.as_ref(), None).ok()?;
    let mut found = None;
    for (scope, id) in candidates {
        let session = session(&id)?;
        if hub.is_some_and(|hub| session.link.hub_name() != *hub)
            || matches!(session.link.status(), Status::Online { .. })
        {
            continue;
        }
        let logins: Vec<AccountName> =
            session.link.view().me().map(|me| me.logins.iter().cloned().collect()).unwrap_or_default();
        let reader = match account {
            Some(account) => account.clone(),
            None => match logins.as_slice() {
                [only] => only.clone(),
                _ => continue,
            },
        };
        if !timeline_copy::read_by(&session.link.view().directory, &scope, &logins, Some(&reader)) {
            continue;
        }
        if found.replace((scope, session, reader)).is_some() {
            return None;
        }
    }
    let (scope, session, reader) = found?;
    let record = crate::records_copy::record_in(replica.library(), &scope.key, ConversationPlan::STREAM, "plan")?;
    Some(crate::records_copy::plan_as_told(&session.link.view().directory, &reader, owner, record))
}

#[cfg(test)]
mod tests {
    use super::{Cut, cut, run_of};
    use fastctx_content::v2::Covered;
    use fastctx_types::timeline_pages::{TimelineEvent, TimelinePlace, TimelineShown};

    fn key(n: u8) -> (Vec<u8>, Vec<u8>) {
        (vec![n], vec![n])
    }

    fn run(low: Option<u8>, high: Option<u8>) -> Covered {
        Covered { low: low.map_or((Vec::new(), Vec::new()), key), high: high.map(key), through: 0, remade: 0 }
    }

    fn event(n: u8) -> TimelineEvent {
        let hex = format!("{n:02x}");
        TimelineEvent {
            place: TimelinePlace { order: hex.clone(), item: hex },
            sequence: u64::from(n),
            likeness: None,
            shown: TimelineShown::Event { event: serde_json::json!({}) },
        }
    }

    #[test]
    fn a_page_begins_in_the_run_that_holds_what_is_just_below_it() {
        // The newest window 180..; an older page 80..100 kept apart, a gap
        // between.
        let runs = vec![run(Some(180), None), run(Some(80), Some(100))];
        assert_eq!(run_of(&runs, None).map(|run| run.low.clone()), Some(key(180)));
        assert_eq!(run_of(&runs, Some(&key(190))).map(|run| run.low.clone()), Some(key(180)));
        assert_eq!(run_of(&runs, Some(&key(100))).map(|run| run.low.clone()), Some(key(80)));
        assert!(run_of(&runs, Some(&key(150))).is_none(), "in the gap: nothing held there");
        assert!(run_of(&runs, Some(&key(180))).is_none(), "at the window's low end: what is below is not held");
    }

    #[test]
    fn what_lies_below_a_gap_is_left_out() {
        let mut events: Vec<_> = [90, 95, 185, 190].into_iter().map(event).collect();
        let Cut::Below(oldest) = cut(&mut events, &key(180)).unwrap() else { panic!("cut") };
        assert_eq!(oldest.order, "b9");
        assert_eq!(events.len(), 2);
        let mut within: Vec<_> = [185, 190].into_iter().map(event).collect();
        assert_eq!(cut(&mut within, &key(180)).unwrap(), Cut::Whole);
        let mut below: Vec<_> = [90, 95].into_iter().map(event).collect();
        assert_eq!(cut(&mut below, &key(180)).unwrap(), Cut::Nothing);
    }
}
