//! A resume launcher loses its right to run when its node disappears.

use crate::Node;
use fastctx_hosts::claude::background::{CommandFailure, Printed};
use std::{
    ffi::OsString,
    process::{Command, Stdio},
    time::Duration,
};

pub(super) fn run(node: &Node, command: &super::Command, args: &[OsString]) -> Result<Printed, CommandFailure> {
    let unstarted = |error: std::io::Error| CommandFailure { message: error.to_string(), started: false };
    let (input, owner) = std::io::pipe().map_err(unstarted)?;
    let mut guard = Command::new(&node.fastctx);
    guard
        .args(["command-guard", "--launcher", "--seconds", "120", "--"])
        .arg(&command.program)
        .args(args)
        .current_dir(&command.cwd)
        .env_clear()
        .envs(command.env.iter().map(|(key, value)| (key, value)))
        .env("FASTCTX_HOME", &node.root)
        .stdin(Stdio::from(input));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        guard.creation_flags(0x0800_0000);
    }
    // The daemon and an accepted background session survive independently;
    // an unaccepted CLI invocation must not be left to start after recovery.
    let output = fastctx_platform::bounded::output_tracked(guard, "Claude wake", Duration::from_secs(123));
    drop(owner);
    let output =
        output.map_err(|failure| CommandFailure { message: failure.error.to_string(), started: failure.started })?;
    let (out, told) =
        (String::from_utf8_lossy(&output.stdout).into_owned(), String::from_utf8_lossy(&output.stderr).into_owned());
    if output.status.success() {
        return Ok(Printed { out, told });
    }
    Err(CommandFailure { message: format!("Claude wake ({}): {out}{told}", output.status), started: true })
}
