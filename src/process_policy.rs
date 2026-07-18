//! Shared process-launch policy for FastCtx-owned non-interactive children.

use std::ffi::OsStr;
use std::process::Command;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Creates a command without allocating or inheriting a console window on Windows.
pub(crate) fn noninteractive_command(program: impl AsRef<OsStr>) -> Command {
    let mut command = Command::new(program);
    apply_noninteractive_policy(&mut command);
    command
}

/// Applies the platform policy to an existing non-interactive command.
pub(crate) fn apply_noninteractive_policy(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;

        command.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    {
        let _ = command;
    }
}

/// Composes extra Windows creation flags without dropping the no-window invariant.
#[cfg(windows)]
pub(crate) const fn noninteractive_creation_flags(additional: u32) -> u32 {
    CREATE_NO_WINDOW | additional
}

#[cfg(test)]
mod tests {
    #[cfg(windows)]
    #[test]
    fn creation_flag_composition_preserves_the_no_window_contract() {
        const INDEPENDENT_CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const CREATE_SUSPENDED: u32 = 0x0000_0004;

        let flags = super::noninteractive_creation_flags(CREATE_SUSPENDED);
        assert_eq!(
            flags & INDEPENDENT_CREATE_NO_WINDOW,
            INDEPENDENT_CREATE_NO_WINDOW
        );
        assert_eq!(flags & CREATE_SUSPENDED, CREATE_SUSPENDED);
    }

    #[cfg(windows)]
    #[test]
    fn noninteractive_child_has_no_console() {
        const PROBE: &str = "FASTCTX_TEST_NO_WINDOW_PROBE";
        if std::env::var_os(PROBE).is_some() {
            use windows_sys::Win32::System::Console::GetConsoleWindow;

            // SAFETY: GetConsoleWindow takes no arguments and returns a borrowed HWND.
            assert!(unsafe { GetConsoleWindow() }.is_null());
            return;
        }

        let status = super::noninteractive_command(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "process_policy::tests::noninteractive_child_has_no_console",
            ])
            .env(PROBE, "1")
            .status()
            .unwrap();
        assert!(status.success());
    }
}
