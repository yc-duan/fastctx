//! Deleting the conversations living here once idle longer than this
//! machine's rules keep them (`super`). Only conversations the World started
//! go, never one the user opened in their own window or brought in; one with
//! anything on its way (a turn, words or controls waiting, a command running
//! here or elsewhere, an answer or an end still owed, a goal going on or to
//! go on, a colleague's answer, a sentinel or a timer it waits on) counts as
//! busy, and so does one whose state cannot be read. A pass every ten minutes
//! deletes a few, a minute apart, the longest idle first: here first, then
//! the hubs are asked to have the rest let go. Each is looked at once more
//! when nothing new can begin in it any more, its rule and idle time read
//! anew. Nothing goes in the hour after the node starts, within a rule's
//! grace, or for a day after this machine's clock jumped ahead.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fastctx_types::access::{Acting, HubAccount, HubId};
use fastctx_types::conversation_retention::Due;
use fastctx_types::residents::{About, Activity};
use fastctx_types::session::control::ControlState;
use fastctx_types::{Address, ResidentId};
use jiff::Timestamp;

use super::Rules;
use crate::Node;

const EVERY: Duration = Duration::from_secs(10 * 60);
/// Nothing goes this long after the node starts.
const SETTLING: Duration = Duration::from_secs(60 * 60);
/// The most conversations one pass deletes, and the wait between two.
const AT_ONCE: usize = 6;
const APART: Duration = Duration::from_secs(60);
/// The most conversations one pass asks whether they are busy, which may
/// ask other machines, and how long one found busy is let be.
const LOOKED_AT_ONCE: usize = 24;
const LET_BE: Duration = Duration::from_secs(6 * 60 * 60);
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

/// The conversations found busy lately, each let be until then.
static LET_BE_UNTIL: Mutex<Option<HashMap<ResidentId, Instant>>> = Mutex::new(None);

pub(crate) fn start(node: &Arc<Node>) {
    let weak = Arc::downgrade(node);
    let started = Instant::now();
    fastctx_platform::task::spawn(async move {
        loop {
            tokio::time::sleep(EVERY).await;
            let Some(node) = weak.upgrade() else { return };
            // The clock is looked at every pass, the settling hour too, so
            // that a jump in it is seen from the first.
            let jumped = jumped();
            if started.elapsed() < SETTLING || jumped {
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

/// Whether `id` was found busy lately; those let be long enough are let go.
fn let_be(id: &ResidentId) -> bool {
    let now = Instant::now();
    let mut found = LET_BE_UNTIL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let found = found.get_or_insert_with(HashMap::new);
    found.retain(|_, until| *until > now);
    found.contains_key(id)
}

fn found_busy(id: &ResidentId) {
    let mut found = LET_BE_UNTIL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    found.get_or_insert_with(HashMap::new).insert(id.clone(), Instant::now() + LET_BE);
}

async fn pass(node: &Arc<Node>) {
    let reading = node.clone();
    let due = match fastctx_platform::task::spawn_blocking(move || -> std::io::Result<Vec<Due>> {
        let rules = super::read(&reading)?;
        if !rules.on() {
            return Ok(Vec::new());
        }
        let now = Timestamp::now();
        let mut due: Vec<Due> = due(&reading, &rules)
            .into_iter()
            .map(|(due, _)| due)
            .filter(|due| due.at.is_some_and(|at| at <= now))
            .collect();
        due.sort_by_key(|due| due.idle_since);
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
    let (mut gone, mut looked) = (0, 0);
    for due in due {
        if gone >= AT_ONCE || looked >= LOOKED_AT_ONCE {
            break;
        }
        if let_be(&due.resident) {
            continue;
        }
        looked += 1;
        if busy(node, &due.resident).await {
            found_busy(&due.resident);
            continue;
        }
        let days = due.days.unwrap_or_default();
        match crate::forget::expired(node, &due.resident, still()).await {
            Ok(()) => {
                node.log.line(format_args!(
                    "deleted {}, idle since {}, as this machine keeps conversations idle {days} days",
                    due.resident, due.idle_since
                ));
                gone += 1;
                tokio::time::sleep(APART).await;
            }
            Err(why) => {
                found_busy(&due.resident);
                node.log.line(format_args!("deleting {}, idle {days} days: {why}", due.resident));
            }
        }
    }
}

/// What a conversation going is asked once nothing new can begin in it,
/// right before its deletion is recorded: nothing on its way here, and its
/// rule and idle time read anew, last.
fn still() -> crate::data::delete::Still {
    Arc::new(|node: Arc<Node>, id: ResidentId| {
        Box::pin(async move {
            if busy_here(&node, &id).await {
                return Err("something of it is on its way".into());
            }
            let rules = super::read(&node)
                .map_err(|error| format!("reading this machine's conversation retention: {error}"))?;
            let due = node.residents.about(&id).and_then(|(about, _)| one(&node, &rules, &id, &about));
            if due.and_then(|due| due.at).is_none_or(|at| at > Timestamp::now()) {
                return Err("it is no longer due to go".into());
            }
            Ok(())
        })
    })
}

/// Since when each conversation the World started here has been idle and
/// when the rules have it go, with whom it works for. Call on a thread that
/// may block.
pub(crate) fn due(node: &Node, rules: &Rules) -> Vec<(Due, Acting)> {
    let mut due = Vec::new();
    for resident in node.residents.everyone() {
        if node.residents.is_deleting(&resident.id) {
            continue;
        }
        let Some((about, _)) = node.residents.about(&resident.id) else { continue };
        if let Some(found) = one(node, rules, &resident.id, &about) {
            due.push((found, about.acting.clone()));
        }
    }
    due
}

/// `due` of the conversation `id`, which `about` is of; none where it never
/// goes so: not started by the World, or brought in from the user's own.
fn one(node: &Node, rules: &Rules, id: &ResidentId, about: &About) -> Option<Due> {
    if !about.world_started || about.native_resumed {
        return None;
    }
    let kept = kept(node, rules, &about.acting);
    // The commands it ran are looked at only where a rule may take it.
    let idle_since = idle_since(node, id, kept.is_some());
    let at = kept.map(|(days, grace)| {
        let at = idle_since + jiff::SignedDuration::from_hours(i64::from(days) * 24);
        grace.map_or(at, |grace| at.max(grace))
    });
    Some(Due { resident: id.clone(), idle_since, days: kept.map(|(days, _)| days), at })
}

/// The days a conversation working for `acting` is kept idle, and before
/// when it does not go, as the rules of the networks it works on have it:
/// on each, its owner's own days, or for an assisted account the fewest of
/// the ceiling and its own; across them, the most, and never where any one
/// keeps it. One working for this machine itself goes by its owner's rule
/// on the only network the machine is on, and never on several.
fn kept(node: &Node, rules: &Rules, acting: &Acting) -> Option<(u32, Option<Timestamp>)> {
    match acting {
        Acting::Here => {
            let rule = rules.of(&sole_network(node)?);
            rule.own_days.map(|days| (days, rule.grace_until))
        }
        Acting::Accounts { accounts } => {
            let mut most: Option<(u32, Option<Timestamp>)> = None;
            for account in accounts {
                let rule = rules.of(&account.hub);
                let (days, grace) = if owns(node, account) {
                    (rule.own_days?, rule.grace_until)
                } else {
                    (rule.assisted(account)?, rule.assisted_grace(account))
                };
                most = Some(most.map_or((days, grace), |(most, until)| (most.max(days), until.max(grace))));
            }
            most
        }
    }
}

/// Whether a conversation working for `acting` is under the rule of the
/// network of `hub`.
pub(crate) fn under(node: &Node, hub: &HubId, acting: &Acting) -> bool {
    match acting {
        Acting::Here => sole_network(node).as_ref() == Some(hub),
        Acting::Accounts { accounts } => accounts.iter().any(|account| account.hub == *hub),
    }
}

/// The network this machine is on, where it is on only one.
fn sole_network(node: &Node) -> Option<HubId> {
    match node.links().as_slice() {
        [link] => Some(link.membership().id.clone()),
        _ => None,
    }
}

/// Whether `account` owns this machine on its network.
pub(crate) fn owns(node: &Node, account: &HubAccount) -> bool {
    node.link_of(&account.hub).is_some_and(|link| {
        let view = link.view();
        view.directory.member(&node.machine()).is_some_and(|member| member.home.as_ref() == Some(&account.account))
    })
}

/// Since when `id` has been idle: its last words or turn, or with `jobs` the
/// last change of a command it ran here, as the command's own record says
/// (its folder changes as their files go, which is no doing of its),
/// whichever came later.
fn idle_since(node: &Node, id: &ResidentId, jobs: bool) -> Timestamp {
    let since = node.residents.now(id).map(|now| now.since);
    let jobs = jobs.then(|| jobs_changed(node, id)).flatten();
    match (since, jobs) {
        (Some(since), Some(jobs)) => since.max(jobs),
        (since, jobs) => since.or(jobs).unwrap_or_else(Timestamp::now),
    }
}

/// When a record of the commands `id` ran here last changed.
fn jobs_changed(node: &Node, id: &ResidentId) -> Option<Timestamp> {
    fastctx_platform::disk::read_dir(node.world.conversation(id).jobs())
        .ok()?
        .flatten()
        .filter(|entry| entry.path().extension().is_some_and(|extension| extension == "json"))
        .filter_map(|entry| entry.metadata().ok()?.modified().ok())
        .max()
        .and_then(|time| Timestamp::try_from(time).ok())
}

/// Whether `id` has anything on its way, here or of others, or cannot be
/// told not to.
pub(crate) async fn busy(node: &Arc<Node>, id: &ResidentId) -> bool {
    crate::lifecycle::held(node, id).await.unwrap_or(true)
        || busy_here(node, id).await
        || crate::turn_evidence::waits(node, id).await.unwrap_or(true)
}

/// Whether anything of `id` is on its way here, or cannot be told not to: a
/// turn, controls waiting or running, words waiting for it of any kind, a
/// goal going on or to go on, or an answer, an end or a command still owed.
async fn busy_here(node: &Arc<Node>, id: &ResidentId) -> bool {
    if node.residents.now(id).is_none_or(|now| now.activity == Activity::Busy || now.turn.is_some()) {
        return true;
    }
    match crate::control_queue::pending(node, id).await {
        Ok(rows) if !rows.iter().any(|row| matches!(row.state, ControlState::Waiting | ControlState::Running)) => {}
        _ => return true,
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
    fastctx_platform::task::spawn_blocking(move || -> std::io::Result<bool> {
        let directory = looking.world.conversation(&owner.resident);
        // Words of any kind count, those waiting for a way in too; context
        // that only rides with a next turn starts none (`lifecycle::held`).
        Ok(crate::inbox::any(&directory, |row| row.delivery.notice.along().is_none())?
            || crate::books::in_flight(&looking, &owner, &directory)?)
    })
    .await
    .map_or(true, |busy| busy.unwrap_or(true))
}
