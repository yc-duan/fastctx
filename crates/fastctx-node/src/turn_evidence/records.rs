//! Existing action, call and job records, selected by their captured turn.

use crate::Node;
use fastctx_types::residents::Action;
use fastctx_types::turn_evidence::JobEvidence;
use fastctx_types::{Address, ResidentId};
use std::io;

pub(super) fn actions(node: &Node, resident: &ResidentId) -> io::Result<Vec<Action>> {
    crate::residents::activity::lines(&node.world.conversation(resident).actions())
}

/// The jobs the resident started here, read without the machine's jobs lock:
/// each record is replaced whole, and one collected while the jobs are read
/// is a job long over, left out as if read a moment later.
pub(super) fn jobs(node: &Node, resident: &ResidentId) -> io::Result<Vec<JobEvidence>> {
    let owner = Address { machine: node.machine(), resident: resident.clone() };
    let names = node.names();
    let mut result = Vec::new();
    for directory in node.world.job_directories(&names, &owner)? {
        let entries = match fastctx_platform::disk::read_dir(directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        for entry in entries {
            let path = entry?.path();
            if path.extension().is_none_or(|extension| extension != "json") {
                continue;
            }
            let record = match fastctx_hands::run::record::read(&path) {
                Ok(record) => record,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            };
            if record.owner.resident().is_some_and(|recorded| super::identity::same(&names, recorded, &owner)) {
                result.push(JobEvidence { source: path.to_string_lossy().into_owned(), record });
            }
        }
    }
    result.sort_by_key(|job| job.record.started_at);
    Ok(result)
}
