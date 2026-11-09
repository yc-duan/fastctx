use super::*;

pub(super) fn dirty(row: &mut Row, task: Task) -> Result<()> {
    let was_blocked = row.state == "blocked";
    if row.task.holding != task.holding {
        row.generation = next(row.generation)?;
        let generation = row.generation;
        *row = Row::new(task);
        row.generation = generation;
    } else {
        if number(task.priority) < number(row.task.priority) {
            row.task.priority = task.priority;
        }
        let newer = task.wanted > row.task.wanted
            || task.wanted == row.task.wanted && task.dependency_revision > row.task.dependency_revision;
        let equal = task.wanted == row.task.wanted && task.dependency_revision == row.task.dependency_revision;
        if equal && task.dependency != row.input_dependency {
            return Err(sql::invalid("the same dependency observation cannot name two different blockers"));
        }
        if newer {
            row.task.wanted = task.wanted;
            row.task.dependency_revision = task.dependency_revision;
            row.input_dependency = task.dependency.clone();
            row.task.dependency = task.dependency;
            row.blocked_attempts = 0;
        }
        if row.state != "leased" && (newer || equal) && (!was_blocked || newer) {
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
        if row.state == "blocked" && (!was_blocked || row.blocked_attempts == 0) {
            defer_blocked(row);
        }
    }
    Ok(())
}

fn defer_blocked(row: &mut Row) {
    let delay = (15 * 60 * 1000_i64).saturating_mul(1_i64 << row.blocked_attempts.min(4)).min(4 * 60 * 60 * 1000);
    row.blocked_attempts = row.blocked_attempts.saturating_add(1);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX));
    row.task.next_due_millis = now.saturating_add(delay);
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
pub(super) fn finish(db: &Connection, row: &mut Row, outcome: Outcome) -> Result<()> {
    let claimed = row.lease_wanted.take().ok_or_else(|| sql::invalid("a leased task has no claimed version"))?;
    let observed =
        row.lease_dependency_revision.take().ok_or_else(|| sql::invalid("lease lacks dependency observation"))?;
    let changed =
        row.lease_dependency_sequence.take().ok_or_else(|| sql::invalid("lease lacks dependency sequence"))?;
    row.owner = None;
    if (row.task.wanted > claimed || row.task.dependency_revision > observed)
        && !matches!(outcome, Outcome::Completed { .. })
    {
        row.task.next_due_millis = 0;
        row.state = if row.task.dependency.is_some() { "blocked" } else { "ready" }.into();
        row.reason = None;
        if row.state == "blocked" {
            defer_blocked(row);
        }
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
            if row.state == "blocked" {
                defer_blocked(row);
            } else {
                row.blocked_attempts = 0;
            }
        }
        Outcome::Retry { next_due_millis, reason } => {
            row.task.next_due_millis = next_due_millis;
            row.state = "ready".into();
            row.reason = Some(reason);
        }
        Outcome::Blocked { dependency, reason } => {
            if dependencies::since(db, &dependency, changed)? {
                row.task.dependency = None;
                row.task.next_due_millis = 0;
                row.state = "ready".into();
                row.reason = None;
            } else {
                row.task.dependency = Some(dependency);
                row.state = "blocked".into();
                row.reason = Some(reason);
                defer_blocked(row);
            }
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
