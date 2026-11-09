//! Update constraints last heard from a membership, retained across offline starts.

use std::path::Path;

use fastctx_types::update::{Network, Policy, Rolled};
use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct UpdatePolicy {
    policy: Policy,
    hub: Option<String>,
}

impl UpdatePolicy {
    pub fn of(network: &Network) -> Self {
        Self { policy: network.policy.clone(), hub: network.hub.clone() }
    }

    pub fn network(&self, key: &str) -> Network {
        let rolled = self
            .policy
            .rollout
            .as_ref()
            .map(|rollout| Rolled { version: rollout.version.clone(), takes: rollout.takes(key) });
        Network { policy: self.policy.clone(), hub: self.hub.clone(), rolled, ..Network::default() }
    }

    pub fn read(directory: &Path) -> std::io::Result<Option<Self>> {
        super::state::read_or(&directory.join("update-policy.json"), None)
    }

    pub fn save(&self, directory: &Path) -> std::io::Result<()> {
        super::state::write(&directory.join("update-policy.json"), self)
    }
}
