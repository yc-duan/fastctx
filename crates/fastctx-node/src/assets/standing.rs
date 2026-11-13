//! What this machine tells each hub of where it stands in syncing, and
//! what it sees of itself for the sync status page (design §28.9): where it
//! stands in each owner library, the bytes it took since it last stood
//! anew, the scopes not yet pulled whole, the objects it lacks and the
//! frames not yet sent. A report goes at the end of a pass when it says
//! something new, at most every ten seconds unless whether this machine is
//! settled changed, and at least every five minutes.

use super::Assets;
use super::session::{ask, blocking};
use fastctx_net::link::Link;
use fastctx_types::access::HubId;
use fastctx_types::assets::{Reply, Request};
use fastctx_types::sync_status::{LocalSync, OwnerStanding, SyncReport, SyncStatus, UNPULLED_NAMED};
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The least time between two reports that only say more of the same.
const SPACING: Duration = Duration::from_secs(10);
/// The most time between two reports.
const EVERY: Duration = Duration::from_secs(300);
/// The span the receiving rate is taken over.
const RATE_OVER: Duration = Duration::from_secs(60);

/// What this machine took lately: when, and how many bytes.
#[derive(Default)]
pub(crate) struct Received {
    taken: VecDeque<(Instant, u64)>,
}

impl Received {
    fn add(&mut self, bytes: u64) {
        let now = Instant::now();
        self.taken.push_back((now, bytes));
        self.forget(now);
    }

    fn forget(&mut self, now: Instant) {
        while self.taken.front().is_some_and(|(at, _)| now.duration_since(*at) > RATE_OVER) {
            self.taken.pop_front();
        }
    }

    /// Bytes a second over the last minute.
    fn rate(&mut self) -> u64 {
        self.forget(Instant::now());
        self.taken.iter().map(|(_, bytes)| bytes).sum::<u64>() / RATE_OVER.as_secs()
    }
}

/// `bytes` of frames of `owner`'s library of `hub` were taken (none: of a
/// scope pulled by itself).
pub(super) fn took(assets: &Assets, hub: &HubId, owner: Option<&str>, bytes: u64) {
    assets.received.lock().expect("received").add(bytes);
    if let Some(owner) = owner {
        *assets.taken.lock().expect("taken").entry((hub.as_str().to_string(), owner.to_string())).or_default() += bytes;
    }
}

/// This machine stands anew in `owner`'s library of `hub`: what it took
/// before counts no more.
pub(super) fn stood(assets: &Assets, hub: &HubId, owner: &str) {
    assets.taken.lock().expect("taken").remove(&(hub.as_str().to_string(), owner.to_string()));
}

/// What this machine says of where it stands with `hub`.
async fn facts(assets: &Assets, hub: &HubId) -> Result<SyncReport, String> {
    let library = &assets.library;
    let theirs = hub.as_str().to_string();
    let (feeds, unpulled, lacking, unsent) = blocking(library, move |library| {
        Ok((library.feeds_of(&theirs)?, library.unpulled_of(&theirs)?, library.lacking_of(&theirs)?, library.unsent_of(&theirs)?))
    })
    .await?;
    let taken = assets.taken.lock().expect("taken").clone();
    let owners = feeds
        .into_iter()
        .map(|fed| OwnerStanding {
            taken: taken.get(&(hub.as_str().to_string(), fed.owner.clone())).copied().unwrap_or(0),
            owner: fed.owner,
            generation: fed.generation,
            change: fed.change,
            at: fed.at,
        })
        .collect();
    Ok(SyncReport {
        owners,
        unpulled: u32::try_from(unpulled.len()).unwrap_or(u32::MAX),
        unpulled_scopes: unpulled.into_iter().take(UNPULLED_NAMED).collect(),
        missing_objects: lacking.0,
        missing_bytes: lacking.1,
        unsent_frames: unsent.0,
        unsent_bytes: unsent.1,
        rate: assets.received.lock().expect("received").rate(),
    })
}

/// Whether nothing is left to do either way.
fn settled(report: &SyncReport) -> bool {
    report.unpulled == 0 && report.missing_objects == 0 && report.unsent_frames == 0 && report.rate == 0
}

/// Tells `hub` where this machine stands, when that is due.
pub(super) async fn report(assets: &Assets, hub: &HubId, link: &Arc<Link>) -> Result<(), String> {
    let report = facts(assets, hub).await?;
    let due = match assets.reported.lock().expect("reported").get(hub) {
        None => true,
        Some((last, at)) => {
            at.elapsed() >= EVERY
                || settled(last) != settled(&report)
                || (*last != report && at.elapsed() >= SPACING)
        }
    };
    if !due {
        return Ok(());
    }
    let Reply::Reported = ask(assets, link, Request::Report { report: report.clone() }).await? else {
        return Err("the hub did not keep the report".into());
    };
    assets.reported.lock().expect("reported").insert(hub.clone(), (report, Instant::now()));
    Ok(())
}

/// How `account`'s machines stand in syncing with `hub` over the last
/// `hours`: what this machine sees itself, and the hub's reckoning when it
/// answers, or why it did not.
pub(crate) async fn status(
    assets: &Assets,
    link: &Arc<Link>,
    account: fastctx_types::AccountName,
    hours: u32,
) -> (LocalSync, Option<Box<SyncStatus>>, Option<String>) {
    let hub = link.membership().id;
    let theirs = hub.as_str().to_string();
    let unsent = blocking(&assets.library, move |library| library.unsent_of(&theirs)).await.unwrap_or((0, 0));
    let cut_off = match link.status() {
        fastctx_net::link::Status::Online { .. } => None,
        fastctx_net::link::Status::Connecting { since, .. } => Some(since.as_millisecond()),
        fastctx_net::link::Status::Stopped { .. } => Some(jiff::Timestamp::now().as_millisecond()),
    };
    let local = LocalSync { machine: link.name(), unsent_frames: unsent.0, unsent_bytes: unsent.1, cut_off };
    if cut_off.is_some() {
        return (local, None, Some("the hub is not reached".into()));
    }
    match ask(assets, link, Request::SyncStatus { account, hours }).await {
        Ok(Reply::SyncStatus { status }) => (local, Some(status), None),
        Ok(other) => (local, None, Some(format!("the hub answered otherwise: {other:?}"))),
        Err(why) => (local, None, Some(why)),
    }
}
