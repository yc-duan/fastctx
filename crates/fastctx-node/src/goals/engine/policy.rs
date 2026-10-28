//! Pure decisions from observed facts. The driver commits each result before executing its work.

use super::words;
use fastctx_types::NoticeId;
use fastctx_types::agents::Fact;
use fastctx_types::goals::runtime::{QuotaSource, Retry, Review, ReviewCause, Task, Verdict, WaitKey, Work};
use fastctx_types::goals::{Goal, Status, Stop, StopKind};
use fastctx_types::turn_evidence::{
    Compacted, Liveness, Quota, StopKind as TurnStop, StopReason, TurnEvidence, WaitHandle,
};
use jiff::Timestamp;
use std::time::Duration;

pub(super) const IDLE_SECONDS: u64 = 10;
pub(super) const REMINDER_SECONDS: u64 = 30 * 60;
/// How often a standing goal that waits is looked at again, in case what
/// was to wake it went away unseen.
pub(super) const STANDING_CHECK_SECONDS: u64 = 60 * 60;
pub(super) const QUOTA_MARGIN_SECONDS: u64 = 30;
pub(super) const RETRY_SECONDS: [u64; 10] = [5, 15, 30, 60, 300, 420, 600, 900, 1800, 3600];
pub(super) const FAILURE_LIMIT_MS: u64 = 6 * 60 * 60 * 1000;
/// The node samples its clock every thirty seconds. A larger gap is not evidence of online time.
pub(super) const ONLINE_GAP_MS: u64 = 90_000;

pub(super) struct Clock {
    pub now: Timestamp,
    pub epoch: String,
    pub online_ms: u64,
}

pub(super) struct Decision {
    pub goal: Goal,
    pub kind: &'static str,
    pub reason: String,
}

pub(super) struct QuotaHold {
    pub source: QuotaSource,
    pub reset: Timestamp,
    pub reason: String,
}

fn after(now: Timestamp, seconds: u64) -> Timestamp {
    now.checked_add(Duration::from_secs(seconds)).unwrap_or(Timestamp::MAX)
}

fn decision(goal: Goal, kind: &'static str, reason: impl Into<String>) -> Decision {
    Decision { goal, kind, reason: reason.into() }
}

fn work(goal: &mut Goal, id: NoticeId, due: Timestamp, task: Task) {
    goal.runtime.pending = Some(Work { id, due, turn: goal.runtime.settled_turn.clone(), task });
}

fn stop(goal: &mut Goal, status: Status, kind: StopKind, reason: String) {
    goal.status = status;
    goal.stop = Some(Stop { kind, reason, resume_at: None });
    goal.claim = None;
    goal.runtime.pending = None;
}

/// How long a goal that waits on something waits before it is looked at again.
pub(super) fn remind_seconds(goal: &Goal) -> u64 {
    if goal.standing { STANDING_CHECK_SECONDS } else { REMINDER_SECONDS }
}

/// A standing goal never completes. Stopped with something left to wake it,
/// it waits quietly; stopped with nothing, it is asked once to leave
/// something, and paused at the second such stop in a row.
fn standing(mut goal: Goal, clock: &Clock, id: NoticeId) -> Decision {
    if !goal.runtime.waiting.is_empty() {
        goal.runtime.unwoken = 0;
        work(&mut goal, id, after(clock.now, STANDING_CHECK_SECONDS), Task::Remind { since: clock.now });
        return decision(goal, "waiting", words::WAITING);
    }
    goal.runtime.unwoken = goal.runtime.unwoken.saturating_add(1);
    if goal.runtime.unwoken >= 2 {
        goal.runtime.unwoken = 0;
        stop(&mut goal, Status::Paused, StopKind::Review, words::STANDING_UNWOKEN.into());
        return decision(goal, "standing_unwoken", words::STANDING_UNWOKEN);
    }
    work(&mut goal, id, clock.now, Task::Tell { text: words::STANDING_REMIND.into(), advice: false });
    decision(goal, "standing_remind", words::STANDING_REMIND)
}

pub(super) fn waiting(handles: &[WaitHandle]) -> Vec<WaitKey> {
    handles
        .iter()
        .filter(|handle| handle.will_wake && !matches!(handle.liveness, Liveness::Ended))
        .map(|handle| WaitKey {
            machine: handle.machine.clone(),
            id: handle.id.clone(),
            kind: Some(handle.kind),
            completion: handle.completion.clone(),
        })
        .collect()
}

/// Stale percentage readings are not evidence that Codex has recovered.
pub(super) fn quota_hold(quota: &Quota) -> Option<QuotaHold> {
    let Fact::Known { value } = &quota.limits else { return None };
    let reset =
        value.windows.iter().filter(|window| window.used_tenths >= 1000).filter_map(|window| window.resets_at).max()?;
    Some(QuotaHold { source: QuotaSource::Host, reset, reason: "A reported usage window is full.".into() })
}

fn quota(mut goal: Goal, hold: QuotaHold, clock: &Clock, id: NoticeId) -> Decision {
    let due = after(hold.reset.max(clock.now), QUOTA_MARGIN_SECONDS);
    goal.status = Status::Paused;
    goal.claim = None;
    goal.stop = Some(Stop { kind: StopKind::Quota, reason: hold.reason.clone(), resume_at: Some(due) });
    goal.runtime.retry = None;
    work(&mut goal, id, due, Task::Quota { source: hold.source });
    decision(goal, "quota_wait", hold.reason)
}

/// Reconcile failure time from monotonic readings. Restart or suspend restarts the retry cadence.
pub(super) fn account_time(retry: &mut Retry, clock: &Clock) -> bool {
    let delta = clock.online_ms.checked_sub(retry.checkpoint_ms);
    let continuous = retry.epoch == clock.epoch && delta.is_some();
    if continuous {
        retry.online_ms = retry.online_ms.saturating_add(delta.unwrap());
    } else {
        retry.failures = 0;
    }
    retry.epoch = clock.epoch.to_string();
    retry.checkpoint_ms = clock.online_ms;
    !continuous
}

pub(super) fn failed(mut goal: Goal, stopped: StopReason, clock: &Clock, id: NoticeId) -> Decision {
    let retry = goal.runtime.retry.get_or_insert_with(|| Retry {
        failures: 0,
        online_ms: 0,
        first_failure_at: clock.now,
        epoch: clock.epoch.to_string(),
        checkpoint_ms: clock.online_ms,
        last: stopped.clone(),
    });
    account_time(retry, clock);
    retry.last = stopped.clone();
    let reason = words::stopped(&stopped);
    if retry.online_ms >= FAILURE_LIMIT_MS {
        let reason = format!("{}\n{reason}", words::RETRY_LIMIT);
        stop(&mut goal, Status::Blocked, StopKind::RetryExhausted, reason.clone());
        return decision(goal, "retry_exhausted", reason);
    }
    let delay = RETRY_SECONDS[(retry.failures as usize).min(RETRY_SECONDS.len() - 1)];
    retry.failures = retry.failures.saturating_add(1);
    goal.status = Status::Active;
    goal.claim = None;
    goal.stop = None;
    work(&mut goal, id, after(clock.now, delay), Task::Retry { stopped });
    decision(goal, "retry_scheduled", reason)
}

pub(super) fn host_refused(mut goal: Goal, reason: String) -> Decision {
    goal.runtime.retry = None;
    stop(&mut goal, Status::Blocked, StopKind::Assistance, reason.clone());
    decision(goal, "host_refused", reason)
}

/// Resume the cadence after downtime; only a failed turn or a due retry checks the limit.
pub(super) fn retry_resumed(current: &Goal, clock: &Clock, id: NoticeId) -> Option<Decision> {
    if !current.active() {
        return None;
    }
    let mut goal = current.clone();
    let retry = goal.runtime.retry.as_mut()?;
    retry.failures = 1;
    let stopped = retry.last.clone();
    work(&mut goal, id, after(clock.now, RETRY_SECONDS[0]), Task::Retry { stopped });
    Some(decision(goal, "retry_resumed", "The machine is back; retrying in five seconds."))
}

/// Process a settled turn only once. The driver must first establish that the host and input are idle.
pub(super) fn ended(
    current: &Goal,
    facts: &TurnEvidence,
    assistance: Option<QuotaHold>,
    clock: &Clock,
    id: NoticeId,
    generation: String,
) -> Option<Decision> {
    if current.retired.is_some() || current.status == Status::Complete {
        return None;
    }
    if !current.active() && current.claim.is_none() {
        return None;
    }
    let ended_at = facts.end_recorded_at.or(facts.ended_at)?;
    let turn = facts.turn.as_ref()?;
    if current.runtime.settled_turn.as_ref() == Some(turn)
        || current.runtime.activated_at.is_some_and(|at| ended_at < at)
    {
        return None;
    }
    let stopped = facts.stop.as_ref()?;
    let mut goal = current.clone();
    if facts.end.as_ref().and_then(|end| end.facts.as_ref()).is_some_and(|facts| facts.world_progress) {
        goal.runtime.retry = None;
    }
    goal.runtime.settled_turn = Some(turn.clone());
    goal.runtime.pending = None;
    if stopped.kind == TurnStop::UserStopped {
        goal.runtime.retry = None;
        stop(&mut goal, Status::Paused, StopKind::User, words::USER_STOPPED.into());
        return Some(decision(goal, "user_stopped", words::USER_STOPPED));
    }
    if stopped.kind == TurnStop::ContextWindowExceeded {
        if goal.runtime.compacted_after.take().is_some() {
            let reason = format!("{}\n{}", words::CONTEXT_AGAIN, words::stopped(stopped));
            stop(&mut goal, Status::Blocked, StopKind::Context, reason.clone());
            return Some(decision(goal, "context_blocked", reason));
        }
        work(&mut goal, id, clock.now, Task::Compact);
        return Some(decision(goal, "compaction_requested", words::stopped(stopped)));
    }
    // Compaction guards exactly the next work turn, including one that fails for another reason.
    goal.runtime.compacted_after = None;
    if let Some(hold) = assistance.or_else(|| facts.quota.as_ref().and_then(quota_hold)) {
        return Some(quota(goal, hold, clock, id));
    }
    if stopped.kind != TurnStop::Completed {
        return Some(failed(goal, stopped.clone(), clock, id));
    }
    goal.runtime.retry = None;
    let handles = waiting(&facts.waiting);

    goal.runtime.waiting = handles;
    if let Some(claim) = &goal.claim {
        let cause = if claim.status == Status::Blocked { ReviewCause::Blocked } else { ReviewCause::Paused };
        return Some(review_requested(goal, cause, clock, id));
    }
    if goal.standing {
        return Some(standing(goal, clock, id));
    }
    if !goal.runtime.waiting.is_empty() {
        work(&mut goal, id, after(clock.now, REMINDER_SECONDS), Task::Remind { since: clock.now });
        return Some(decision(goal, "waiting", words::WAITING));
    }
    work(&mut goal, id, after(clock.now, IDLE_SECONDS), Task::Idle { generation });
    Some(decision(goal, "idle_window", words::IDLE))
}

pub(super) fn review_requested(mut goal: Goal, cause: ReviewCause, clock: &Clock, id: NoticeId) -> Decision {
    if goal.runtime.advice_sent >= 2 {
        let reason = goal
            .runtime
            .last_review
            .as_ref()
            .map(|review| words::review(review.cause, &review.verdict))
            .unwrap_or_else(|| words::REVIEW_EXHAUSTED.into());
        let status = match cause {
            ReviewCause::Blocked => Status::Blocked,
            _ => Status::Paused,
        };
        stop(&mut goal, status, StopKind::Review, reason.clone());
        return decision(goal, "review_limit", reason);
    }
    work(&mut goal, id, clock.now, Task::Review { cause, failures: 0, unavailable: None });
    decision(goal, "review_requested", format!("Reviewing the {cause:?} stop."))
}

/// `idle` includes the final host, inbox, composer, and verified-wait recheck.
pub(super) fn reviewed(
    current: &Goal,
    expected: &NoticeId,
    verdict: Verdict,
    observed: Option<(Timestamp, NoticeId)>,
    idle: bool,
    clock: &Clock,
    id: NoticeId,
) -> Option<Decision> {
    let pending = current.runtime.pending.as_ref().filter(|pending| &pending.id == expected)?;
    let Task::Review { cause, .. } = pending.task else { return None };
    let mut goal = current.clone();
    goal.runtime.pending = None;
    if cause != ReviewCause::Paused
        && !matches!(verdict, Verdict::Failed { .. })
        && let Some((through, snapshot)) = observed
    {
        goal.runtime.reviewed_through = Some(through);
        goal.runtime.reviewed_work = Some(snapshot);
    }
    let reason = words::review(cause, &verdict);
    goal.runtime.last_review = Some(Review { at: clock.now, cause, verdict: verdict.clone(), delivered: false });
    match verdict {
        Verdict::Sound { .. } => {
            goal.runtime.advice_sent = 0;
            if idle {
                let status = match cause {
                    ReviewCause::Blocked => Status::Blocked,
                    _ => Status::Paused,
                };
                stop(&mut goal, status, StopKind::Review, reason.clone());
            }
            Some(decision(goal, if idle { "review_sound" } else { "review_obsolete" }, reason))
        }
        Verdict::Advice { .. } if idle => {
            goal.status = Status::Active;
            goal.stop = None;
            goal.claim = None;
            work(&mut goal, id, clock.now, Task::Tell { text: reason.clone(), advice: true });
            Some(decision(goal, "review_advice", reason))
        }
        Verdict::Advice { .. } => Some(decision(goal, "review_obsolete", reason)),
        Verdict::Failed { .. } => {
            if idle {
                stop(&mut goal, Status::Paused, StopKind::Review, reason.clone());
            }
            Some(decision(goal, "review_failed", reason))
        }
    }
}

/// No sentinel can look: the stop stands as the agent left it, a blocked
/// one blocked, any other paused, and the user is told why.
pub(super) fn unreviewed(current: &Goal, expected: &NoticeId, reason: String, idle: bool) -> Option<Decision> {
    let pending = current.runtime.pending.as_ref().filter(|pending| &pending.id == expected)?;
    let Task::Review { cause, .. } = pending.task else { return None };
    let mut goal = current.clone();
    goal.runtime.pending = None;
    let reason = format!("{}\n{reason}", words::UNREVIEWED);
    if idle {
        let status = if cause == ReviewCause::Blocked { Status::Blocked } else { Status::Paused };
        stop(&mut goal, status, StopKind::Review, reason.clone());
    }
    Some(decision(goal, if idle { "unreviewed" } else { "review_obsolete" }, reason))
}

/// Offline homes do not consume attempts. Provider failures get the initial attempt and two retries.
pub(super) fn review_unavailable(
    current: &Goal,
    expected: &NoticeId,
    offline: bool,
    reason: String,
    clock: &Clock,
    id: NoticeId,
) -> Option<Decision> {
    let pending = current.runtime.pending.as_ref().filter(|pending| &pending.id == expected)?;
    let Task::Review { cause, failures, .. } = pending.task else { return None };
    let mut goal = current.clone();
    if !offline && failures >= 2 {
        let reason = format!("{}\n{reason}", words::REVIEW_FAILED);
        stop(&mut goal, Status::Paused, StopKind::Review, reason.clone());
        goal.runtime.last_review = Some(Review {
            at: clock.now,
            cause,
            verdict: Verdict::Failed { reason: reason.clone() },
            delivered: false,
        });
        return Some(decision(goal, "review_failed", reason));
    }
    let failures = if offline { failures } else { failures + 1 };
    work(
        &mut goal,
        id,
        after(clock.now, if offline { 30 } else { RETRY_SECONDS[failures as usize - 1] }),
        Task::Review { cause, failures, unavailable: Some(reason.clone()) },
    );
    Some(decision(goal, if offline { "review_offline" } else { "review_retry" }, reason))
}

pub(super) fn compacted(
    current: &Goal,
    expected: &NoticeId,
    outcome: Compacted,
    clock: &Clock,
    id: NoticeId,
) -> Option<Decision> {
    let pending = current
        .runtime
        .pending
        .as_ref()
        .filter(|pending| &pending.id == expected && matches!(pending.task, Task::Compact))?;
    let mut goal = current.clone();
    match outcome {
        Compacted::Done => {
            goal.runtime.compacted_after = pending.turn.clone();
            goal.status = Status::Active;
            goal.stop = None;
            goal.claim = None;
            work(&mut goal, id, clock.now, Task::Tell { text: words::COMPACTED.into(), advice: false });
            Some(decision(goal, "compacted", words::COMPACTED))
        }
        Compacted::Failed { reason } | Compacted::Refused { reason } => {
            stop(&mut goal, Status::Blocked, StopKind::Context, reason.clone());
            Some(decision(goal, "context_blocked", reason))
        }
    }
}

/// Called after durable custody of a suggestion; replay cannot count it twice.
pub(super) fn delivered(current: &Goal, expected: &NoticeId) -> Option<Decision> {
    let pending = current.runtime.pending.as_ref().filter(|pending| &pending.id == expected)?;
    let mut goal = current.clone();
    if matches!(pending.task, Task::Tell { advice: true, .. })
        && current
            .runtime
            .last_review
            .as_ref()
            .is_some_and(|review| !review.delivered && matches!(review.verdict, Verdict::Advice { .. }))
    {
        goal.runtime.advice_sent = goal.runtime.advice_sent.saturating_add(1).min(2);
        goal.runtime.last_review.as_mut().unwrap().delivered = true;
    }
    goal.runtime.pending = None;
    Some(decision(goal, "delivered", "The scheduled goal message entered the conversation's durable inbox."))
}

/// A fresh permission from Codex is required even when a reset deadline has passed.
pub(super) fn quota_ready(
    current: &Goal,
    expected: &NoticeId,
    assistance: Option<QuotaHold>,
    reading: Option<&Quota>,
    codex: bool,
    clock: &Clock,
    id: NoticeId,
) -> Option<Decision> {
    current
        .runtime
        .pending
        .as_ref()
        .filter(|pending| &pending.id == expected && matches!(pending.task, Task::Quota { .. }))?;
    if let Some(hold) = assistance {
        return Some(quota(current.clone(), hold, clock, id));
    }
    let allowed = reading.and_then(|reading| reading.ordinary_usage_allowed);
    if (!codex || allowed != Some(true))
        && let Some(hold) = reading.and_then(quota_hold).filter(|hold| hold.reset > clock.now)
    {
        return Some(quota(current.clone(), hold, clock, id));
    }
    if codex && allowed != Some(true) {
        return Some(failed(
            current.clone(),
            StopReason {
                kind: TurnStop::UsageLimited,
                category: Some(serde_json::json!({"ordinaryUsageAllowed": allowed})),
                message: Some(words::QUOTA_UNKNOWN.into()),
            },
            clock,
            id,
        ));
    }
    let mut goal = current.clone();
    goal.status = Status::Active;
    goal.stop = None;
    goal.claim = None;
    work(&mut goal, id, clock.now, Task::Tell { text: words::QUOTA_RECOVERED.into(), advice: false });
    Some(decision(goal, "quota_resumed", words::QUOTA_RECOVERED))
}

/// Disappearance from a wait list is insufficient: the caller has verified this result arrived.
pub(super) fn wait_returned(current: &Goal, handle: &WaitKey) -> Option<Decision> {
    if !current.runtime.waiting.contains(handle) {
        return None;
    }
    let mut goal = current.clone();
    goal.runtime.waiting.retain(|waiting| waiting != handle);
    goal.runtime.advice_sent = 0;
    goal.runtime.unwoken = 0;
    if goal.runtime.pending.as_ref().is_some_and(|work| matches!(work.task, Task::Remind { .. } | Task::Idle { .. })) {
        goal.runtime.pending = None;
    }
    Some(decision(goal, "wait_returned", format!("{} on {} returned.", handle.id, handle.machine)))
}

/// The caller authenticates the speaker as the user or the requesting conversation.
pub(super) fn owner_spoke(current: &Goal, input: &str) -> Option<Decision> {
    if current.runtime.owner_input.as_deref() == Some(input) {
        return None;
    }
    let mut goal = current.clone();
    goal.runtime.owner_input = Some(input.into());
    goal.runtime.advice_sent = 0;
    goal.runtime.unwoken = 0;
    if goal.runtime.pending.as_ref().is_some_and(|pending| {
        matches!(pending.task, Task::Idle { .. } | Task::Review { .. } | Task::Tell { advice: true, .. })
    }) {
        goal.runtime.pending = None;
    }
    Some(decision(goal, "owner_spoke", "The goal's owner spoke; consecutive suggestions reset."))
}

/// A turn's acceptance alone is not evidence that it ran normally.
pub(super) fn healthy(current: &Goal) -> Option<Decision> {
    current.runtime.retry.as_ref()?;
    let mut goal = current.clone();
    goal.runtime.retry = None;
    Some(decision(goal, "work_resumed", "The host reported work after starting the turn; consecutive failures reset."))
}

/// An opened goal starts work even if the conversation has never had a turn.
pub(super) fn activated(current: &Goal, clock: &Clock, id: NoticeId) -> Option<Decision> {
    if !current.active() || current.runtime.pending.is_some() || current.runtime.settled_turn.is_some() {
        return None;
    }
    let mut goal = current.clone();
    work(&mut goal, id, clock.now, Task::Tell { text: words::OPENED.into(), advice: false });
    Some(decision(goal, "activation", words::OPENED))
}

pub(super) fn idle_ready(
    current: &Goal,
    expected: &NoticeId,
    generation: &str,
    idle: bool,
    clock: &Clock,
    id: NoticeId,
) -> Option<Decision> {
    let pending = current.runtime.pending.as_ref().filter(|pending| &pending.id == expected)?;
    let Task::Idle { generation: original } = &pending.task else { return None };
    if pending.due > clock.now {
        return None;
    }
    if !idle || original != generation {
        let mut goal = current.clone();
        goal.runtime.pending = None;
        return Some(decision(
            goal,
            "idle_cancelled",
            "The conversation did not remain idle throughout the observation window.",
        ));
    }
    if current.standing {
        let mut goal = current.clone();
        goal.runtime.pending = None;
        goal.runtime.waiting.clear();
        return Some(standing(goal, clock, id));
    }
    Some(review_requested(current.clone(), ReviewCause::Idle, clock, id))
}

pub(super) fn reminded(
    current: &Goal,
    expected: &NoticeId,
    handles: &[WaitHandle],
    idle: bool,
    generation: String,
    clock: &Clock,
    id: NoticeId,
) -> Option<Decision> {
    let pending = current.runtime.pending.as_ref().filter(|pending| &pending.id == expected)?;
    let Task::Remind { since } = pending.task else { return None };
    if pending.due > clock.now {
        return None;
    }
    let mut goal = current.clone();
    goal.runtime.pending = None;
    if !idle {
        return Some(decision(goal, "reminder_cancelled", "The conversation resumed before its waiting reminder."));
    }
    goal.runtime.waiting = waiting(handles);
    if goal.standing {
        return Some(standing(goal, clock, id));
    }
    if goal.runtime.waiting.is_empty() {
        work(&mut goal, id, after(clock.now, IDLE_SECONDS), Task::Idle { generation });
        return Some(decision(goal, "idle_window", words::IDLE));
    }
    let text = words::reminder(since, clock.now, handles);
    work(&mut goal, id, clock.now, Task::Tell { text: text.clone(), advice: false });
    Some(decision(goal, "waiting_reminder", text))
}
