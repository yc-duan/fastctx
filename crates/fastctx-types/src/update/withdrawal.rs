//! A withdrawn release cannot be selected, acquired or installed, however requested.
use super::{FailureReason, Policy, Problem, State};

impl Policy {
    /// Rejects a version withdrawn by this network's administrator.
    pub fn check_release(&self, version: &str) -> Result<(), Problem> {
        if self.withdrawn.contains(version) {
            return Err(Problem::new(
                FailureReason::Withdrawn,
                format!("the network's administrator withdrew {version}"),
            ));
        }
        Ok(())
    }
}

impl State {
    /// Applies the merged withdrawal policy, including explicit local requests.
    pub fn check_release(&self, version: &str) -> Result<(), Problem> {
        if let Some(network) = &self.network {
            network.policy.check_release(version)?;
        }
        Ok(())
    }
}
