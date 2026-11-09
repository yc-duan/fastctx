//! Scope and source cleanup outlive the transport holding they are retiring.
use super::*;
use crate::store::source_step::StepKey;

pub(crate) fn scope_changed(db: &Connection, scope: &ScopeKey, generation: LocalSequence) -> Result<()> {
    dirty(
        db,
        TaskKey { kind: "sync.reconcile".into(), scope: scope.clone(), key: crate::blocks::reconcile::task_key(scope) },
        generation,
    )
}
pub(crate) fn source_cleanup(db: &Connection, step: &StepKey) -> Result<()> {
    dirty(
        db,
        TaskKey {
            kind: "sync.reconcile".into(),
            scope: step.scope.clone(),
            key: format!("source-step/{}/{}", step.source.as_str(), step.revision.0),
        },
        LocalSequence(1),
    )
}
fn dirty(db: &Connection, key: TaskKey, wanted: LocalSequence) -> Result<()> {
    let existing = get(db, &key)?;
    if existing.as_ref().is_some_and(|row| row.task.wanted >= wanted) {
        return Ok(());
    }
    let task = Task {
        key,
        wanted,
        completed: LocalSequence(0),
        priority: Priority::Control,
        next_due_millis: 0,
        dependency: None,
        dependency_revision: wanted,
        dependency_sequence: dependencies::sequence(db)?,
        holding: None,
    };
    let mut row = existing.unwrap_or_else(|| Row::new(task.clone()));
    update::dirty(db, &mut row, task)?;
    save(db, &row)
}
