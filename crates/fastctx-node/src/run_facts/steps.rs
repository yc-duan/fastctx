//! The steps a run's commands tell (`fastctx_types::run_facts::RunStep`),
//! kept beside its job record, one JSON row each, and sent on with its
//! facts. A step told again under its id, as an `on` that began and then
//! ended, is a later row that replaces the earlier when read.

use super::*;
use fastctx_types::run_facts::{RunStep, merge_steps};

fn log_path(record: &Path) -> std::path::PathBuf {
    record.with_extension("steps.jsonl")
}

/// Keeps `step` with the job `record` describes and tells its owner.
pub(crate) fn tell(node: &Node, record: &Path, step: RunStep) -> io::Result<()> {
    if !step.bounded() {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "a step beyond what a run's facts carry"));
    }
    {
        let _writing = WRITING.lock();
        let mut row = serde_json::to_vec(&step)?;
        row.push(b'\n');
        log::append(&log_path(record), &row)?;
    }
    super::update_job_with(node, record, Vec::new(), vec![step])
}

/// What the job `record` describes was told of its steps, each step as
/// last told.
pub(crate) fn steps(record: &Path) -> io::Result<Vec<RunStep>> {
    let _writing = WRITING.lock();
    let mut steps = Vec::new();
    log::read(&log_path(record), |row| {
        let step: RunStep = serde_json::from_slice(row).map_err(io::Error::other)?;
        merge_steps(&mut steps, std::slice::from_ref(&step));
        Ok(())
    })?;
    Ok(steps)
}
