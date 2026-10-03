//! Processes as the World records them.

use serde::{Deserialize, Serialize};

/// One lifetime of a process. A PID alone does not name one: the system
/// reuses PIDs. The token changes whenever the PID is reused (on Linux it
/// is the boot and the start time, on Windows the creation time); it is
/// compared, stored and shown, never parsed.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProcessIdentity {
    pub pid: u32,
    pub token: String,
}
