//! Compacting a conversation idle a while (Issue #57): the user's `/compact`
//! pressed for them once a conversation the World runs has been idle as long
//! as the setting of its login says for the account it works for, or as the
//! user said of it alone (`About::idle_compaction`); off unless turned on.
//! Claude Code and Codex alike, colleagues too; the agents a host runs inside
//! a turn are not conversations of their own.
//!
//! A sweep looks at the conversations here every half minute. One is
//! compacted when it has been idle since its last turn ended for as long as
//! the setting says, holds at least the context it says, has nothing waiting
//! to go in and no goal under way, and its last turn was not a compaction.
//! Each idle stretch is compacted at most once: it is marked before the
//! compaction is asked for, and a turn that says nothing, as a compaction's
//! own does whether it worked or not, begins none, so one that fails is not
//! tried again until the conversation speaks again. What the hosts said of
//! context and cache is kept in memory: after the node starts, a
//! conversation is looked at once it has taken a turn again. Where Claude Code said how long its cache
//! lasts, a stretch past that, or too near it, is let be: compacting reads
//! the whole conversation again, and would cost more than it saves.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use fastctx_types::ResidentId;
use fastctx_types::residents::Activity;
use fastctx_types::session::{Happened, SessionChange};
use fastctx_types::session::control::Control;
use jiff::Timestamp;

use crate::Node;

/// How often the conversations are looked at.
const SWEEP: Duration = Duration::from_secs(30);
/// The most compacting at once on this machine, and in one login.
const AT_ONCE: usize = 2;
const OF_ONE_LOGIN: usize = 1;
/// How long one asked for is counted as under way at most, its end unseen.
const UNDER_WAY: Duration = Duration::from_secs(15 * 60);
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

/// The compactions asked for and not yet seen through: each conversation's
/// login, and when it was asked.
static UNDER_WAY_NOW: LazyLock<Mutex<HashMap<ResidentId, (PathBuf, Instant)>>> = LazyLock::new(Default::default);

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
    // While no account turned it on for any login here, only a conversation
    // told to on its own is compacted.
    let any = crate::logins::idle::any_on(node);
    let candidates: Vec<Candidate> =
        node.residents.everyone().filter_map(|record| candidate(node, record, now, any)).collect();
    if candidates.is_empty() {
        return;
    }
    // Those asked for before and seen through, or long enough ago that they
    // are not waited for, make room again.
    let asked: Vec<ResidentId> = UNDER_WAY_NOW.lock().expect("never poisoned").keys().cloned().collect();
    for id in asked {
        let through = match crate::lifecycle::held(node, &id).await {
            Ok(held) => !held,
            Err(_) => true,
        };
        let mut under_way = UNDER_WAY_NOW.lock().expect("never poisoned");
        if through || under_way.get(&id).is_some_and(|(_, at)| at.elapsed() >= UNDER_WAY) {
            under_way.remove(&id);
        }
    }
    for candidate in candidates {
        {
            let under_way = UNDER_WAY_NOW.lock().expect("never poisoned");
            if under_way.len() >= AT_ONCE {
                return;
            }
            let of_login =
                under_way.values().filter(|(login, _)| crate::homes::same_dir(login, &candidate.login)).count();
            if under_way.contains_key(&candidate.id) || of_login >= OF_ONE_LOGIN {
                continue;
            }
        }
        // Nothing waits to go in: no control, no words handed over.
        match crate::lifecycle::held(node, &candidate.id).await {
            Ok(false) => {}
            Ok(true) => continue,
            Err(error) => {
                node.log.line(format_args!("idle compaction of {}: {}", candidate.id, error.message));
                continue;
            }
        }
        // Marked before it is asked for: a stretch is compacted at most once.
        if let Err(error) = mark(node, &candidate.id, &candidate.after).await {
            node.log.line(format_args!("idle compaction of {}: marking it: {error}", candidate.id));
            continue;
        }
        UNDER_WAY_NOW
            .lock()
            .expect("never poisoned")
            .insert(candidate.id.clone(), (candidate.login.clone(), Instant::now()));
        node.log.line(format_args!("compacting {}, idle since its turn {}", candidate.id, candidate.after));
        if let Err(error) = crate::control::control_here(node, &candidate.id, Control::Compact).await {
            node.log.line(format_args!("idle compaction of {}: {}", candidate.id, error.message));
            UNDER_WAY_NOW.lock().expect("never poisoned").remove(&candidate.id);
        }
    }
}

fn candidate(node: &Node, record: crate::residents::OnRecord, now: Timestamp, any: bool) -> Option<Candidate> {
    let crate::residents::OnRecord { removed, id, about, now: live, here, .. } = record;
    let agent = about.host.agent.as_str();
    if removed.is_some() || !here || !about.world_managed() || !matches!(agent, "claude" | "codex") {
        return None;
    }
    if !any && about.idle_compaction != Some(true) {
        return None;
    }
    let live = live?;
    // Its last turn compacted it, or said nothing, as one that compacted
    // and failed does: no idle stretch of its own follows.
    if live.activity == Activity::Busy
        || live.turn.is_some()
        || live.last_compacted
        || live.last_said.is_none()
        || node.residents.is_deleting(&id)
    {
        return None;
    }
    let after = live.last_turn.clone()?;
    if node.goals.views.lock().expect("never poisoned").get(&id).is_some_and(|view| view.goal.as_ref().is_some_and(|goal| goal.active())) {
        return None;
    }
    let login = crate::usage::login_of(node, &id, &about)?;
    let accounts = crate::logins::idle::accounts_of(node, &about.acting);
    let setting = crate::logins::idle::of(node, &accounts, agent, &login).unwrap_or_default();
    if !about.idle_compaction.unwrap_or(setting.on) {
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
    if marked(node, &id).as_deref() == Some(after.as_str()) {
        return None;
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
