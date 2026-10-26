//! Goal transitions operate on a proposed record; only a successful account CAS makes them real.

use super::words;
use fastctx_types::Address;
use fastctx_types::access::HubAccount;
use fastctx_types::goals::{Author, Claim, Decision, Goal, Mark, Owner, Status, Stop, StopKind, UserChange};
use fastctx_types::ipc::Failure;
use jiff::Timestamp;

pub(crate) struct Change<'a> {
    pub operation: &'a str,
    pub author: Author,
    pub turn: Option<String>,
    pub at: Timestamp,
}

pub(crate) fn open(
    resident: Address,
    account: HubAccount,
    owner: Owner,
    objective: String,
    change: Change<'_>,
) -> Result<Goal, Failure> {
    nonempty(&objective, words::BLANK_GOAL)?;
    let mut goal = Goal {
        id: crate::speech::new_id("goal").to_string(),
        resident,
        account,
        objective,
        owner,
        status: Status::Active,
        created_at: change.at,
        created_by: change.author.clone(),
        changed_at: change.at,
        stop: None,
        statement: None,
        claim: None,
        retired: None,
        decisions: Vec::new(),
        history: None,
        runtime: fastctx_types::goals::runtime::Runtime {
            activated_at: Some(change.at),
            activation: Some(change.operation.into()),
            ..Default::default()
        },
        operation: change.operation.to_string(),
    };
    remember(&mut goal, change, "opened", String::new());
    Ok(goal)
}

pub(crate) fn user(goal: &mut Goal, requested: UserChange, change: Change<'_>) -> Result<(), Failure> {
    if goal.retired.is_some() && !matches!(requested, UserChange::Open { .. } | UserChange::Resume) {
        return Err(Failure::not_run(words::RETIRED));
    }
    match requested {
        UserChange::Open { objective } => {
            nonempty(&objective, words::BLANK_GOAL)?;
            goal.objective = objective;
            activate(goal, change.at, change.operation);
            remember(goal, change, "opened", String::new());
        }
        UserChange::Edit { objective } => {
            nonempty(&objective, words::BLANK_GOAL)?;
            goal.objective = objective;
            if goal.runtime.pending.as_ref().is_some_and(|pending| {
                matches!(
                    pending.task,
                    fastctx_types::goals::runtime::Task::Idle { .. }
                        | fastctx_types::goals::runtime::Task::Review { .. }
                        | fastctx_types::goals::runtime::Task::Tell { advice: true, .. }
                )
            }) {
                goal.runtime.pending = None;
            }
            remember(goal, change, "edited", String::new());
        }
        UserChange::Resume => {
            activate(goal, change.at, change.operation);
            remember(goal, change, "resumed", String::new());
        }
        UserChange::Pause => {
            let reason = if matches!(change.author, Author::Resident { .. }) {
                words::REQUESTER_PAUSED
            } else {
                words::USER_PAUSED
            };
            goal.status = Status::Paused;
            goal.stop = Some(Stop { kind: StopKind::User, reason: reason.into(), resume_at: None });
            goal.claim = None;
            goal.runtime.pending = None;
            goal.runtime.retry = None;
            remember(goal, change, "paused", reason.into());
        }
        UserChange::Clear => return Err(Failure::new("Clear deletes the account record.")),
    }
    Ok(())
}

pub(crate) fn mark(goal: &mut Goal, mark: Mark, change: Change<'_>) -> Result<(), Failure> {
    if goal.retired.is_some() {
        return Err(Failure::not_run(words::RETIRED));
    }
    let (status, statement, kind, reason) = match mark {
        Mark::Complete { statement } => {
            nonempty(&statement, words::BLANK_STATEMENT)?;
            (Status::Complete, Some(statement.clone()), "completed", statement)
        }
        Mark::Blocked { statement } => {
            nonempty(&statement, words::BLANK_STATEMENT)?;
            (Status::Blocked, Some(statement.clone()), "blocked", statement)
        }
        Mark::Pause => (Status::Paused, None, "pause_claimed", words::AGENT_PAUSED.into()),
    };
    goal.status = status;
    goal.runtime.pending = None;
    goal.statement = statement;
    goal.stop =
        (status != Status::Complete).then(|| Stop { kind: StopKind::Claimed, reason: reason.clone(), resume_at: None });
    goal.claim = (status != Status::Complete).then(|| Claim { status, turn: change.turn.clone(), at: change.at });
    if goal.claim.is_some() {
        goal.runtime.settled_turn = None;
    }
    remember(goal, change, kind, reason);
    Ok(())
}

fn activate(goal: &mut Goal, at: Timestamp, operation: &str) {
    goal.runtime = fastctx_types::goals::runtime::Runtime {
        activated_at: Some(at),
        activation: Some(operation.into()),
        ..Default::default()
    };
    goal.retired = None;
    goal.status = Status::Active;
    goal.stop = None;
    goal.statement = None;
    goal.claim = None;
}

pub(crate) fn remember(goal: &mut Goal, change: Change<'_>, kind: &str, reason: String) {
    super::reports::remember(goal, change.operation, change.at);
    goal.operation = change.operation.to_string();
    goal.changed_at = change.at;
    goal.decisions.push(Decision {
        operation: change.operation.to_string(),
        at: change.at,
        by: change.author,
        kind: kind.into(),
        turn: change.turn,
        reason,
    });
}

fn nonempty(text: &str, message: &str) -> Result<(), Failure> {
    if text.trim().is_empty() { Err(Failure::not_run(message)) } else { Ok(()) }
}
