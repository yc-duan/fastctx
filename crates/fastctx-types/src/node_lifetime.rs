//! The node's own observation of its process lifetime, never a promise
//! inferred from how its starter asked Windows to create it.

use serde::{Deserialize, Serialize};

use crate::ProcessIdentity;

/// The immediate Windows job. Ancestor jobs may impose additional limits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowsJob {
    pub kill_on_close: bool,
    pub breakaway_allowed: bool,
    pub silent_breakaway: bool,
}

/// The client that automatically started a node. This is provenance, not
/// an authorization to terminate or otherwise control that process.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeStarter {
    pub process: ProcessIdentity,
    pub session: Option<crate::residents::Host>,
}

/// Runtime facts about this incarnation of the node.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeLifetime {
    pub process: ProcessIdentity,
    pub starter: Option<NodeStarter>,
    pub supervised: bool,
    pub windows_job: Option<WindowsJob>,
}

impl NodeLifetime {
    /// A restriction callers need before relying on background work.
    /// Service guardians intentionally own their nodes' lifetime.
    pub fn warning(&self) -> Option<String> {
        let job = self.windows_job.as_ref().filter(|_| !self.supervised)?;
        let origin = match &self.starter {
            Some(starter) => match &starter.session {
                Some(host) => format!("started by a {} session (process {})", host.client, starter.process.pid),
                None => format!("started by process {}", starter.process.pid),
            },
            None => "started outside a node service".into(),
        };
        let end = if job.kill_on_close {
            "when that job closes, as it does when the session that holds it ends"
        } else {
            "if that job is ended"
        };
        Some(format!(
            "this machine's fastctx node (process {}) was {origin} and runs inside a Windows job: {end}, the node and the jobs it runs end with it; a fastctx node service would keep them apart",
            self.process.pid
        ))
    }
}
