//! Shell-free npm command construction shared by discovery and update transactions.

use super::model::{NpmDriver, NpmProvenance};
use std::path::Path;
use std::process::Command;

/// Creates the exact npm command represented by trusted launcher provenance.
pub(super) fn command(provenance: &NpmProvenance) -> Command {
    match provenance.driver {
        NpmDriver::NodeScript => {
            let mut command = crate::process_policy::console_inheriting_command(&provenance.node);
            command.arg(&provenance.npm_cli);
            command
        }
        NpmDriver::Executable => {
            crate::process_policy::console_inheriting_command(&provenance.npm_cli)
        }
    }
}

/// Creates the same npm command under the shared background-child policy.
pub(super) fn noninteractive_command(provenance: &NpmProvenance) -> Command {
    let mut command = command(provenance);
    crate::process_policy::apply_noninteractive_policy(&mut command);
    command
}

/// Program path named when spawning the represented npm command fails.
pub(super) fn program(provenance: &NpmProvenance) -> &Path {
    match provenance.driver {
        NpmDriver::NodeScript => &provenance.node,
        NpmDriver::Executable => &provenance.npm_cli,
    }
}
