//! The steps a run's commands tell (`fastctx_types::run_facts::RunStep`),
//! kept beside its job record, one JSON row each, and sent on with its
//! facts. A step told again under its id, as an `on` that began and then
//! ended, is a later row that replaces the earlier when read.

use super::*;
use fastctx_types::run_facts::RunStep;

/// Keeps `step` with the job `record` describes and tells its owner.
pub(crate) fn tell(node: &Node, record: &Path, step: RunStep) -> io::Result<()> {
    if !step.bounded() {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "a step beyond what a run's facts carry"));
    }
    let job = fastctx_hands::run::record::read(record)?;
    let book_segment = crate::activity_book::segment(node, &job)?;
    let emits = job.call_id.is_some() && job.owner.resident().is_some();
    let sent = if emits { super::updates::segment(record)?.min(book_segment) } else { book_segment };
    {
        let gate = WRITING.get(&record.with_extension("facts"));
        let _writing = gate.lock();
        let mut row = serde_json::to_vec(&step)?;
        row.push(b'\n');
        super::journal::append(record, &row, sent)?;
    }
    super::update_job(node, record)
}

/// What the job `record` describes was told of its steps, each step as
/// last told.
pub(crate) fn steps(record: &Path) -> io::Result<Vec<RunStep>> {
    let gate = WRITING.get(&record.with_extension("facts"));
    let _writing = gate.lock();
    let mut steps = Vec::new();
    let mut positions = std::collections::HashMap::<String, usize>::new();
    super::journal::read(record, |row| {
        let step: RunStep = serde_json::from_slice(row).map_err(io::Error::other)?;
        if let Some(&index) = positions.get(&step.id) {
            if step.supersedes(&steps[index]) {
                steps[index] = step;
            }
        } else {
            positions.insert(step.id.clone(), steps.len());
            steps.push(step);
        }
        Ok(())
    })?;
    Ok(steps)
}
