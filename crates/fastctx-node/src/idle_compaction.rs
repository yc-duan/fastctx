//! Compacting a conversation idle a while (Issue #57): once a conversation
//! the World runs has been idle as long as the setting of its login says for
//! the account it works for, each part of it overridden where the user set
//! that part for the conversation alone (`About::idle_compaction`), the World
//! compacts it; off unless turned on. Claude Code and Codex alike, colleagues
//! too; the agents a host runs inside a turn are not conversations of their
//! own.
//!
//! A sweep looks at the conversations here every minute. One is compacted
//! when it has been idle since its last turn ended for as long as the
//! setting says, holds at least the context it says, has
//! nothing waiting to go in and no goal under way, its last turn said
//! something, and a Claude Code one still has its host. It is the user's
//! `/compact` with every check that passes (`control::compact_idle`), but it
//! never waits in the conversation's line: under its action gate, one that
//! finds anything under way or waiting, or the conversation no longer idle as
//! it was, is not made; one that fails is not tried again, nor holds up the
//! user's words after it. Each idle stretch is compacted at most once: it is
//! marked before the compaction begins, and a turn that says nothing, as a
//! compaction's own does, begins none. Nor is one compacted that was
//! compacted since its host last answered, by the user or anyone else, as
//! the host's own record shows (`compacted`): a Claude Code compaction is no
//! turn, and leaves the stretch and the context heard of it as they were.
//!
//! What the hosts said of context and cache is kept in memory: after the
//! node starts, a conversation is looked at once it has taken a turn again.
//! Where Claude Code said how long its cache lasts, a stretch past that, or
//! too near it, is let be: compacting reads the whole conversation again,
//! and would cost more than it saves.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use fastctx_types::ResidentId;
use fastctx_types::residents::Activity;
use fastctx_types::session::{Happened, SessionChange};
use fastctx_types::session::control::Controlled;
use jiff::Timestamp;

use crate::Node;

mod compacted;

/// How often the conversations are looked at: as often as those whose
/// hosts idle are let go of (`lifecycle::retention`).
const SWEEP: Duration = Duration::from_secs(60);
/// The most compacting at once on this machine, and in one login.
const AT_ONCE: usize = 2;
const OF_ONE_LOGIN: usize = 1;
/// How long a compaction holds its place at most, should its end never be
/// heard: longer than a compaction is waited for (`control_queue::COMPACTING`).
const UNDER_WAY: Duration = Duration::from_secs(45 * 60);
/// How long before its cache lapses a compaction must begin at the latest.
const BEFORE_LAPSE: Duration = Duration::from_secs(5 * 60);

/// What the host last said of a conversation: how much context it holds,
/// and how long the cache its last request wrote lasts, with when.
#[derive(Default)]
struct Heard {
    context: Option<u64>,
    cache: Option<(Duration, Timestamp)>,
}

static HEARD: LazyLock<Mutex<HashMap<ResidentId, Heard>>> = LazyLock::new(Default::default);

/// A place among the compactions at once: its conversation's login, when it
/// was taken, and whether its compaction began; one not begun yet is let go
/// only by its own task.
struct Place {
    login: PathBuf,
    taken: Instant,
    begun: bool,
}

static UNDER_WAY_NOW: LazyLock<Mutex<HashMap<ResidentId, Place>>> = LazyLock::new(Default::default);

/// Takes in what a host reported of `id`'s session.
pub(crate) fn heard(id: &ResidentId, happened: &Happened, at: Timestamp) {
    match happened {
        Happened::Context { used, .. } => {
            HEARD.lock().expect("never poisoned").entry(id.clone()).or_default().context = Some(*used);
        }
        // Claude Code's request as its transcript keeps it: what it wrote
        // to the cache says for how long, one hour or five minutes; one that
        // only read it keeps it for as long again.
        Happened::Changed { change: SessionChange::MessageUsage { usage, api_error: false, .. }, .. } => {
            let tokens = |key: &str| usage["cache_creation"][key].as_u64().unwrap_or(0);
            let lasts = if tokens("ephemeral_1h_input_tokens") > 0 {
                Some(Duration::from_secs(60 * 60))
            } else if tokens("ephemeral_5m_input_tokens") > 0 {
                Some(Duration::from_secs(5 * 60))
            } else {
                None
            };
            let read = usage["cache_read_input_tokens"].as_u64().unwrap_or(0) > 0;
            let mut all = HEARD.lock().expect("never poisoned");
            let heard = all.entry(id.clone()).or_default();
            match (lasts, heard.cache) {
                (Some(lasts), _) => heard.cache = Some((lasts, at)),
                (None, Some((lasts, _))) if read => heard.cache = Some((lasts, at)),
                _ => {}
            }
        }
        _ => {}
    }
}

pub(crate) fn start(node: &Arc<Node>) {
    let node = node.clone();
    fastctx_platform::task::spawn(async move {
        let mut timer = tokio::time::interval(SWEEP);
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            timer.tick().await;
            sweep(&node).await;
        }
    });
}

/// One that may be compacted now, as the cheap facts in memory say.
struct Candidate {
    id: ResidentId,
    login: PathBuf,
    /// The turn the stretch it has been idle since followed.
    after: String,
}

async fn sweep(node: &Arc<Node>) {
    let now = Timestamp::now();
    let any = crate::logins::idle::any_on(node);
    let candidates: Vec<Candidate> =
        node.residents.everyone().filter_map(|record| candidate(node, record, now, any)).collect();
    // What was heard of conversations gone goes with them.
    let heard: Vec<ResidentId> = HEARD.lock().expect("never poisoned").keys().cloned().collect();
    let gone: Vec<ResidentId> = heard.into_iter().filter(|id| node.residents.about(id).is_none()).collect();
    if !gone.is_empty() {
        HEARD.lock().expect("never poisoned").retain(|id, _| !gone.contains(id));
    }
    // Those begun and through make room again: nothing of theirs under way
    // or waiting any more, or taken long enough ago not to be waited for.
    let begun: Vec<ResidentId> =
        UNDER_WAY_NOW.lock().expect("never poisoned").iter().filter(|(_, place)| place.begun).map(|(id, _)| id.clone()).collect();
    for id in begun {
        let through = !crate::lifecycle::held(node, &id).await.unwrap_or(false);
        let mut under_way = UNDER_WAY_NOW.lock().expect("never poisoned");
        if through {
            under_way.remove(&id);
        }
    }
    UNDER_WAY_NOW.lock().expect("never poisoned").retain(|_, place| place.taken.elapsed() < UNDER_WAY);
    for found in candidates {
        {
            let under_way = UNDER_WAY_NOW.lock().expect("never poisoned");
            if under_way.len() >= AT_ONCE {
                return;
            }
            let of_login =
                under_way.values().filter(|place| crate::homes::same_dir(&place.login, &found.login)).count();
            if under_way.contains_key(&found.id) || of_login >= OF_ONE_LOGIN {
                continue;
            }
        }
        // Marked before it begins: a stretch is compacted at most once, made
        // or not.
        if let Err(error) = mark(node, &found.id, &found.after).await {
            node.log.line(format_args!("idle compaction of {}: marking it: {error}", found.id));
            continue;
        }
        UNDER_WAY_NOW
            .lock()
            .expect("never poisoned")
            .insert(found.id.clone(), Place { login: found.login.clone(), taken: Instant::now(), begun: false });
        let (node, id, after) = (node.clone(), found.id, found.after);
        fastctx_platform::task::spawn(async move {
            // Looked at again under the conversation's action gate: what was
            // true a moment ago in the sweep may not be now. Its record is
            // read there, once, and last: a compaction the user asked for
            // just before is in it by then.
            let (looking, again, before) = (node.clone(), id.clone(), after.clone());
            let still = move || {
                let now = looking.residents.on_record(&again).and_then(|record| idle(&looking, record, Timestamp::now(), any));
                if now.is_none_or(|now| now.after != before) {
                    return Err("it is no longer idle as it was".to_string());
                }
                compacted::since_answer(&looking, &again)
            };
            node.log.line(format_args!("compacting {id}, idle since its turn {after}"));
            let begun = match crate::control::compact_idle(&node, &id, still).await {
                Ok(Controlled::Refused { said, .. }) => {
                    node.log.line(format_args!("idle compaction of {id} not made: {said}"));
                    false
                }
                Ok(_) => true,
                Err(error) => {
                    node.log.line(format_args!("idle compaction of {id} failed: {}", error.message));
                    false
                }
            };
            // One begun holds its place until nothing of it is under way.
            let mut under_way = UNDER_WAY_NOW.lock().expect("never poisoned");
            if begun {
                if let Some(place) = under_way.get_mut(&id) {
                    place.begun = true;
                }
            } else {
                under_way.remove(&id);
            }
        });
    }
}

/// `idle`, of a stretch not compacted yet.
fn candidate(node: &Node, record: crate::residents::OnRecord, now: Timestamp, any: bool) -> Option<Candidate> {
    let found = idle(node, record, now, any)?;
    (marked(node, &found.id).as_deref() != Some(found.after.as_str())).then_some(found)
}

/// One idle long enough to compact now, as its record and what its host
/// said have it, whether its stretch was compacted or not.
fn idle(node: &Node, record: crate::residents::OnRecord, now: Timestamp, any: bool) -> Option<Candidate> {
    let crate::residents::OnRecord { removed, id, about, now: live, here, .. } = record;
    let agent = about.host.agent.as_str();
    if removed.is_some() || !here || !about.world_managed() || !matches!(agent, "claude" | "codex") {
        return None;
    }
    if !any && about.idle_compaction.on != Some(true) {
        return None;
    }
    let live = live?;
    // Its last turn compacted it, or said nothing, as a compaction's own
    // does: no idle stretch of its own follows.
    if live.activity == Activity::Busy
        || live.turn.is_some()
        || live.last_compacted
        || live.last_said.is_none()
        || node.residents.is_deleting(&id)
    {
        return None;
    }
    // A Claude Code conversation put to sleep, or let go of as it idled,
    // has no host to compact in, nor any cache left worth keeping.
    if agent == "claude" && !node.residents.host_present(&id) {
        return None;
    }
    let after = live.last_turn.clone()?;
    if node.goals.views.lock().expect("never poisoned").get(&id).is_some_and(|view| view.goal.as_ref().is_some_and(|goal| goal.active())) {
        return None;
    }
    let login = crate::usage::login_of(node, &id, &about)?;
    let accounts = crate::logins::idle::accounts_of(node, &about.acting);
    let setting = about.idle_compaction.over(crate::logins::idle::of(node, &accounts, agent, &login).unwrap_or_default());
    if !setting.on {
        return None;
    }
    let idle = Duration::from_secs(now.as_second().saturating_sub(live.since.as_second()).max(0) as u64);
    if idle < Duration::from_secs(u64::from(setting.after_minutes) * 60) {
        return None;
    }
    {
        let all = HEARD.lock().expect("never poisoned");
        let heard = all.get(&id)?;
        if heard.context? < setting.min_tokens {
            return None;
        }
        if let Some((lasts, at)) = heard.cache {
            let lapses = at.as_second().saturating_add(lasts.as_secs() as i64);
            if now.as_second().saturating_add(BEFORE_LAPSE.as_secs() as i64) >= lapses {
                return None;
            }
        }
    }
    Some(Candidate { id, login, after })
}

/// The turn the last idle stretch compacted followed, as kept in the
/// conversation's directory.
fn marked(node: &Node, id: &ResidentId) -> Option<String> {
    let path = node.world.conversation(id).idle_compacted();
    let bytes = fastctx_platform::durable_file::read_bounded(&path, 4096).ok()?;
    String::from_utf8(bytes).ok().map(|turn| turn.trim().to_string())
}

async fn mark(node: &Node, id: &ResidentId, turn: &str) -> std::io::Result<()> {
    let path = node.world.conversation(id).idle_compacted();
    let bytes = turn.as_bytes().to_vec();
    fastctx_platform::task::spawn_blocking(move || {
        fastctx_platform::publish(&path, &bytes, fastctx_platform::Permissions::Private)
    })
    .await
    .map_err(std::io::Error::other)?
}
