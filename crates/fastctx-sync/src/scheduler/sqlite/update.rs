use super::*;

pub(super) fn dirty(row: &mut Row, task: Task) -> Result<()> {
    if row.task.holding != task.holding {
        row.generation = next(row.generation)?;
        row.task = task;
        row.task.completed = LocalSequence(0);
        row.state = "ready".into();
        row.owner = None;
        row.attempts = 0;
    } else {
        row.task.wanted = row.task.wanted.max(task.wanted);
        if number(task.priority) < number(row.task.priority) {
            row.task.priority = task.priority;
        }
        row.task.dependency = task.dependency;
        if row.state != "leased" {
            row.task.next_due_millis = row.task.next_due_millis.min(task.next_due_millis);
        }
    }
    if row.state != "leased" {
        row.state = if row.task.completed >= row.task.wanted {
            "complete"
        } else if row.task.dependency.is_some() {
            "blocked"
        } else {
            "ready"
        }
        .into();
        row.reason = None;
    }
    Ok(())
}
pub(super) fn validate_outcome(outcome: &Outcome) -> std::result::Result<(), Error> {
    let reason = match outcome {
        Outcome::Retry { reason, .. } | Outcome::Blocked { reason, .. } | Outcome::Failed { reason } => Some(reason),
        _ => None,
    };
    if reason.is_some_and(|reason| reason.len() > 4096) {
        return Err(Error::Invalid("task outcome reason exceeds its bound".into()));
    }
    if let Outcome::Blocked { dependency, .. } = outcome
        && (dependency.is_empty() || dependency.len() > 1024)
    {
        return Err(Error::Invalid("invalid task dependency".into()));
    }
    Ok(())
}
pub(super) fn finish(row: &mut Row, outcome: Outcome) -> Result<()> {
    let claimed = row.lease_wanted.take().ok_or_else(|| sql::invalid("a leased task has no claimed version"))?;
    row.owner = None;
    if row.task.wanted > claimed && !matches!(outcome, Outcome::Completed { .. }) {
        row.task.next_due_millis = 0;
        row.state = if row.task.dependency.is_some() { "blocked" } else { "ready" }.into();
        row.reason = None;
        return Ok(());
    }
    match outcome {
        Outcome::Completed { through } => {
            if through > claimed {
                return Err(sql::invalid("a worker cannot complete a version it did not claim"));
            }
            row.task.completed = row.task.completed.max(through);
            row.task.next_due_millis = 0;
            row.state = if row.task.completed >= row.task.wanted {
                "complete"
            } else if row.task.dependency.is_some() {
                "blocked"
            } else {
                "ready"
            }
            .into();
            row.reason = None;
        }
        Outcome::Retry { next_due_millis, reason } => {
            row.task.next_due_millis = next_due_millis;
            row.state = "ready".into();
            row.reason = Some(reason);
        }
        Outcome::Blocked { dependency, reason } => {
            row.task.dependency = Some(dependency);
            row.state = "blocked".into();
            row.reason = Some(reason);
        }
        Outcome::Superseded => {
            row.state = "superseded".into();
            row.reason = None;
        }
        Outcome::Failed { reason } => {
            row.state = "failed".into();
            row.reason = Some(reason);
        }
    }
    Ok(())
}
