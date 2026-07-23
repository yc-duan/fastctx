//! Shell-free npm command construction shared by discovery and update transactions.

use super::model::{NpmDriver, NpmProvenance};
use std::path::Path;
use std::process::Command;

/// Creates the exact npm command represented by trusted launcher provenance.
pub(super) fn command(provenance: &NpmProvenance) -> Command {
    match provenance.driver {
        NpmDriver::NodeScript => {
            let mut command = Command::new(&provenance.node);
            command.arg(&provenance.npm_cli);
            command
        }
        NpmDriver::Executable => Command::new(&provenance.npm_cli),
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

#[cfg(test)]
mod tests {
    use super::{command, noninteractive_command, program};
    use crate::update::model::{NpmDriver, NpmMode, NpmProvenance};
    use std::ffi::OsStr;
    use std::path::PathBuf;

    fn provenance(driver: NpmDriver) -> NpmProvenance {
        NpmProvenance {
            package: "fastctx".to_string(),
            mode: NpmMode::Global,
            node: PathBuf::from("/runtime/node"),
            driver,
            npm_cli: PathBuf::from("/runtime/npm-cli"),
            launcher: PathBuf::from("/packages/fastctx/launcher.js"),
            launcher_pid: 42,
            handoff_file: PathBuf::from("/update/npm-launcher-42.handoff"),
        }
    }

    #[test]
    fn node_script_uses_node_with_the_cli_as_its_first_argument() {
        let provenance = provenance(NpmDriver::NodeScript);
        let command = noninteractive_command(&provenance);
        assert_eq!(command.get_program(), OsStr::new("/runtime/node"));
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            vec![OsStr::new("/runtime/npm-cli")]
        );
        assert_eq!(program(&provenance), provenance.node);
    }

    #[test]
    fn executable_driver_is_never_passed_to_node() {
        let provenance = provenance(NpmDriver::Executable);
        let command = command(&provenance);
        assert_eq!(command.get_program(), OsStr::new("/runtime/npm-cli"));
        assert_eq!(command.get_args().count(), 0);
        assert_eq!(program(&provenance), provenance.npm_cli);
    }
}
