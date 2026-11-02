//! Existing action, call and job records, selected by their captured turn.

use crate::Node;
use fastctx_types::residents::Action;
use fastctx_types::turn_evidence::JobEvidence;
use fastctx_types::{Address, ResidentId};
use std::io;

pub(super) fn actions(node: &Node, resident: &ResidentId) -> io::Result<Vec<Action>> {
    crate::residents::activity::lines(&node.world.conversation(resident).actions())
}

pub(super) fn jobs(node: &Node, resident: &ResidentId) -> io::Result<Vec<JobEvidence>> {
    let owner = Address { machine: node.machine(), resident: resident.clone() };
    let names = node.names();
    let _disk = node.disk.lock();
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
            let record = fastctx_hands::run::record::read(&path)?;
            if record.owner.resident().is_some_and(|recorded| super::identity::same(&names, recorded, &owner)) {
                result.push(JobEvidence { source: path.to_string_lossy().into_owned(), record });
            }
        }
    }
    result.sort_by_key(|job| job.record.started_at);
    Ok(result)
}
