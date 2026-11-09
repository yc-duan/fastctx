use super::*;

pub(super) const COLUMNS: &str = "kind,hub,scope,task_key,wanted,completed,state,priority,next_due,lease_generation,lease_owner,attempts,dependency,reason,holding,lease_wanted,dependency_revision,input_dependency,lease_dependency_revision,lease_dependency_sequence,blocked_attempts,dependency_sequence";

fn optional_counter(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<Option<LocalSequence>> {
    if matches!(row.get_ref(index)?, rusqlite::types::ValueRef::Null) {
        Ok(None)
    } else {
        sql::counter(row, index).map(LocalSequence).map(Some)
    }
}

pub(super) struct Row {
    pub task: Task,
    pub state: String,
    pub generation: Revision,
    pub owner: Option<OperationId>,
    pub attempts: i64,
    pub reason: Option<String>,
    pub lease_wanted: Option<LocalSequence>,
    pub input_dependency: Option<String>,
    pub lease_dependency_revision: Option<LocalSequence>,
    pub lease_dependency_sequence: Option<LocalSequence>,
    pub blocked_attempts: u32,
}
impl Row {
    pub fn new(mut task: Task) -> Self {
        task.completed = LocalSequence(0);
        Self {
            input_dependency: task.dependency.clone(),
            task,
            state: "ready".into(),
            generation: Revision(0),
            owner: None,
            attempts: 0,
            reason: None,
            lease_wanted: None,
            lease_dependency_revision: None,
            lease_dependency_sequence: None,
            blocked_attempts: 0,
        }
    }
}
fn parsed<T: TryFrom<String, Error = String>>(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<T> {
    T::try_from(row.get::<_, String>(index)?).map_err(|why| {
        rusqlite::Error::FromSqlConversionFailure(index, rusqlite::types::Type::Text, Box::new(sql::invalid(why)))
    })
}
pub(super) fn read(row: &rusqlite::Row<'_>) -> rusqlite::Result<Row> {
    let holding: Option<Vec<u8>> = row.get(14)?;
    let owner: Option<String> = row.get(10)?;
    Ok(Row {
        task: Task {
            key: TaskKey {
                kind: row.get(0)?,
                scope: ScopeKey { hub: parsed::<HubId>(row, 1)?, scope: parsed::<ScopeId>(row, 2)? },
                key: row.get(3)?,
            },
            wanted: LocalSequence(sql::counter(row, 4)?),
            completed: LocalSequence(sql::counter(row, 5)?),
            priority: match row.get::<_, u8>(7)? {
                0 => Priority::Control,
                1 => Priority::Interactive,
                2 => Priority::Visible,
                3 => Priority::Background,
                _ => return Err(rusqlite::Error::InvalidQuery),
            },
            next_due_millis: row.get(8)?,
            dependency: row.get(12)?,
            dependency_revision: LocalSequence(sql::counter(row, 16)?),
            dependency_sequence: LocalSequence(sql::counter(row, 21)?),
            holding: holding.map(|bytes| serde_json::from_slice(&bytes)).transpose().map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(14, rusqlite::types::Type::Blob, Box::new(error))
            })?,
        },
        input_dependency: row.get(17)?,
        lease_dependency_revision: optional_counter(row, 18)?,
        lease_dependency_sequence: optional_counter(row, 19)?,
        blocked_attempts: row.get(20)?,
        state: row.get(6)?,
        generation: Revision(sql::counter(row, 9)?),
        owner: owner.map(OperationId::try_from).transpose().map_err(|why| {
            rusqlite::Error::FromSqlConversionFailure(10, rusqlite::types::Type::Text, Box::new(sql::invalid(why)))
        })?,
        attempts: row.get(11)?,
        reason: row.get(13)?,
        lease_wanted: if matches!(row.get_ref(15)?, rusqlite::types::ValueRef::Null) {
            None
        } else {
            Some(LocalSequence(sql::counter(row, 15)?))
        },
    })
}
pub(super) fn get(db: &Connection, key: &TaskKey) -> Result<Option<Row>> {
    Ok(db.prepare_cached("SELECT kind,hub,scope,task_key,wanted,completed,state,priority,next_due,lease_generation,lease_owner,attempts,dependency,reason,holding,lease_wanted,dependency_revision,input_dependency,lease_dependency_revision,lease_dependency_sequence,blocked_attempts,dependency_sequence FROM sync_tasks WHERE kind=?1 AND hub=?2 AND scope=?3 AND task_key=?4")?.query_row(params![key.kind,key.scope.hub.to_string(),key.scope.scope.as_str(),key.key],read).optional()?)
}
pub(super) fn save(db: &Connection, row: &Row) -> Result<()> {
    let task = &row.task;
    let holding =
        task.holding.as_ref().map(serde_json::to_vec).transpose().map_err(|error| sql::invalid(error.to_string()))?;
    db.prepare_cached("INSERT INTO sync_tasks(kind,hub,scope,task_key,wanted,completed,state,priority,next_due,lease_generation,lease_owner,attempts,dependency,reason,holding,lease_wanted,dependency_revision,input_dependency,lease_dependency_revision,lease_dependency_sequence,blocked_attempts,dependency_sequence) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22) ON CONFLICT(kind,hub,scope,task_key) DO UPDATE SET wanted=excluded.wanted,completed=excluded.completed,state=excluded.state,priority=excluded.priority,next_due=excluded.next_due,lease_generation=excluded.lease_generation,lease_owner=excluded.lease_owner,attempts=excluded.attempts,dependency=excluded.dependency,reason=excluded.reason,holding=excluded.holding,lease_wanted=excluded.lease_wanted,dependency_revision=excluded.dependency_revision,input_dependency=excluded.input_dependency,lease_dependency_revision=excluded.lease_dependency_revision,lease_dependency_sequence=excluded.lease_dependency_sequence,blocked_attempts=excluded.blocked_attempts,dependency_sequence=excluded.dependency_sequence")?.execute(params![task.key.kind,task.key.scope.hub.to_string(),task.key.scope.scope.as_str(),task.key.key,task.wanted.0.to_be_bytes().as_slice(),task.completed.0.to_be_bytes().as_slice(),row.state,number(task.priority),task.next_due_millis,row.generation.0.to_be_bytes().as_slice(),row.owner.as_ref().map(OperationId::as_str),row.attempts,task.dependency,row.reason,holding,if row.state == "leased" { row.lease_wanted.map(|value|value.0.to_be_bytes()) } else { None }, task.dependency_revision.0.to_be_bytes().as_slice(),row.input_dependency,if row.state=="leased" {row.lease_dependency_revision.map(|v|v.0.to_be_bytes())} else {None},if row.state=="leased" {row.lease_dependency_sequence.map(|v|v.0.to_be_bytes())} else {None},row.blocked_attempts,task.dependency_sequence.0.to_be_bytes().as_slice()])?;
    Ok(())
}
