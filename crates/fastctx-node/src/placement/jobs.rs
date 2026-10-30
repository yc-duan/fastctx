//! Withdrawal waits for every tracked writer, including jobs without an account
//! attribution and jobs launched before their conversation machine was renamed.
use crate::Node;
use fastctx_types::{Address, ipc::Failure, layout::Here as _, run::JobOwner};
pub(crate) fn busy(node: &Node, owners: &[Address]) -> Result<bool, Failure> {
    let names = node.names();
    for owner in owners {
        let mut aliases = names.names_of(&owner.machine);
        if !aliases.contains(&owner.machine) {
            aliases.push(owner.machine.clone());
        }
        for directory in node.world.job_directories(&names, owner).map_err(|error| Failure::new(error.to_string()))? {
            let entries = match std::fs::read_dir(directory) {
                Ok(entries) => entries,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(Failure::new(error.to_string())),
            };
            for entry in entries {
                let path = entry.map_err(|error| Failure::new(error.to_string()))?.path();
                if path.extension().is_none_or(|extension| extension != "json") {
                    continue;
                }
                let job = fastctx_hands::run::record::read(&path).map_err(|error| Failure::new(error.to_string()))?;
                if job.end.is_none()
                    && matches!(job.owner,JobOwner::Resident(address)if address.resident==owner.resident&&aliases.contains(&address.machine))
                {
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}
