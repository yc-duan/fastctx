//! Deleting the conversations living here once idle longer than this
//! machine's rule keeps them (`super`). Only conversations the World runs go,
//! never one the user opened in their own window; one with anything on its
//! way (a turn, words or controls waiting, a command running here or
//! elsewhere, an answer or an end still owed, a goal going on or to go on)
//! counts as busy, and so does one whose state cannot be read. A pass every
//! ten minutes deletes a few, a minute apart, the longest idle first: here
//! first, then the hubs are asked to have the rest let go. Nothing goes in
//! the hour after the node starts, within a rule's grace, or for a day after
//! this machine's clock jumped ahead.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fastctx_types::access::{Acting, HubAccount};
use fastctx_types::conversation_retention::{Due, Rule};
use fastctx_types::{Address, ResidentId};
use jiff::Timestamp;

use crate::Node;

const EVERY: Duration = Duration::from_secs(10 * 60);
/// Nothing goes this long after the node starts.
const SETTLING: Duration = Duration::from_secs(60 * 60);
/// The most conversations one pass deletes, and the wait between two.
const AT_ONCE: usize = 6;
const APART: Duration = Duration::from_secs(60);
/// A jump of the clock this far ahead of the time that passed stops
/// deletions for a day.
const JUMP: jiff::SignedDuration = jiff::SignedDuration::from_hours(24);

/// Where the clock stood at the last pass, and until when nothing goes after
/// a jump.
struct Clock {
    wall: Timestamp,
    steady: Instant,
    held_until: Option<Instant>,
}

static CLOCK: Mutex<Option<Clock>> = Mutex::new(None);

pub(crate) fn start(node: &Arc<Node>) {
    let weak = Arc::downgrade(node);
    let started = Instant::now();
    fastctx_platform::task::spawn(async move {
        loop {
            tokio::time::sleep(EVERY).await;
            let Some(node) = weak.upgrade() else { return };
            if started.elapsed() < SETTLING || jumped() {
                continue;
            }
            pass(&node).await;
        }
    });
}

/// Whether the clock jumped ahead lately, so that idle times read now are
/// not to be trusted.
fn jumped() -> bool {
    let (wall, steady) = (Timestamp::now(), Instant::now());
    let mut clock = CLOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let held = match clock.as_ref() {
        Some(last) => {
            let passed = jiff::SignedDuration::try_from(steady.duration_since(last.steady)).unwrap_or(JUMP);
            if wall.duration_since(last.wall) > passed + JUMP {
                Some(steady + Duration::from_secs(24 * 60 * 60))
            } else {
                last.held_until.filter(|until| *until > steady)
            }
        }
        None => None,
    };
    *clock = Some(Clock { wall, steady, held_until: held });
    held.is_some()
}

async fn pass(node: &Arc<Node>) {
    let reading = node.clone();
    let due = match fastctx_platform::task::spawn_blocking(move || -> std::io::Result<Vec<(Due, Acting)>> {
        let rule = super::read(&reading)?;
        if !rule.on() || rule.grace_until.is_some_and(|grace| grace > Timestamp::now()) {
            return Ok(Vec::new());
        }
        let mut due = due(&reading, &rule);
        due.retain(|(due, _)| due.at.is_some_and(|at| at <= Timestamp::now()));
        due.sort_by_key(|(due, _)| due.idle_since);
        Ok(due)
    })
    .await
    {
        Ok(Ok(due)) => due,
        Ok(Err(error)) => {
            node.log.line(format_args!("reading this machine's conversation retention: {error}"));
            return;
        }
        Err(error) => {
            node.log.line(format_args!("reading this machine's conversation retention: {error}"));
            return;
        }
    };
    let mut gone = 0;
    for (due, _) in due {
        if gone >= AT_ONCE {
            break;
        }
        if busy(node, &due.resident).await {
            continue;
        }
        let days = due.days.unwrap_or_default();
        match crate::forget::expired(node, &due.resident).await {
            Ok(()) => {
                node.log.line(format_args!(
                    "deleted {}, idle since {}, as this machine keeps conversations idle {days} days",
                    due.resident, due.idle_since
                ));
                gone += 1;
                tokio::time::sleep(APART).await;
            }
            Err(why) => node.log.line(format_args!("deleting {}, idle {days} days: {why}", due.resident)),
        }
    }
}

/// Since when each conversation the World runs here has been idle and when
/// the rule has it go, with whom it works for. Call on a thread that may
/// block.
pub(crate) fn due(node: &Node, rule: &Rule) -> Vec<(Due, Acting)> {
    let mut due = Vec::new();
    for resident in node.residents.everyone() {
        let Some((about, _)) = node.residents.about(&resident.id) else { continue };
        if !about.world_managed() || node.residents.is_deleting(&resident.id) {
            continue;
        }
        let days = days_of(node, rule, &about.acting);
        let idle_since = idle_since(node, &resident.id);
        let at = days.map(|days| {
            let at = idle_since + jiff::SignedDuration::from_hours(i64::from(days) * 24);
            rule.grace_until.map_or(at, |grace| at.max(grace))
        });
        due.push((Due { resident: resident.id.clone(), idle_since, days, at }, about.acting.clone()));
    }
    due
}

/// The days a conversation working for `acting` is kept idle: the owner's
/// own, or for an assisted account the fewest of the ceiling and its own.
fn days_of(node: &Node, rule: &Rule, acting: &Acting) -> Option<u32> {
    match acting {
        Acting::Here => rule.own_days,
        Acting::Accounts { accounts } if accounts.iter().any(|account| owns(node, account)) => rule.own_days,
        Acting::Accounts { accounts } => accounts.iter().filter_map(|account| rule.assisted(account)).min(),
    }
}

/// Whether `account` owns this machine on its hub.
fn owns(node: &Node, account: &HubAccount) -> bool {
    node.link_of(&account.hub).is_some_and(|link| {
        let view = link.view();
        view.directory.member(&node.machine()).is_some_and(|member| member.home.as_ref() == Some(&account.account))
    })
}

/// Since when `id` has been idle: its last words or turn, or the last change
/// of the commands it ran here, whichever came later.
fn idle_since(node: &Node, id: &ResidentId) -> Timestamp {
    let since = node.residents.now(id).map(|now| now.since);
    let jobs = fastctx_platform::disk::metadata(node.world.conversation(id).jobs())
        .ok()
        .and_then(|metadata| metadata.modified().ok())
        .and_then(|time| Timestamp::try_from(time).ok());
    match (since, jobs) {
        (Some(since), Some(jobs)) => since.max(jobs),
        (since, jobs) => since.or(jobs).unwrap_or_else(Timestamp::now),
    }
}

/// Whether `id` has anything on its way, or cannot be told not to.
async fn busy(node: &Arc<Node>, id: &ResidentId) -> bool {
    if crate::lifecycle::held(node, id).await.unwrap_or(true) {
        return true;
    }
    if node
        .goals
        .views
        .lock()
        .expect("goal views")
        .get(id)
        .is_some_and(|view| view.pending.is_some() || view.goal.as_ref().is_some_and(crate::goals::waiting::continues))
    {
        return true;
    }
    let (looking, owner) = (node.clone(), Address { machine: node.machine(), resident: id.clone() });
    fastctx_platform::task::spawn_blocking(move || {
        let directory = looking.world.conversation(&owner.resident);
        crate::books::in_flight(&looking, &owner, &directory)
    })
    .await
    .map_or(true, |owes| owes.unwrap_or(true))
}
