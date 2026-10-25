//! Retirement commands cannot outlive the node's conversation gates.

use crate::Node;
use std::{
    ffi::OsString,
    process::{Command, Stdio},
    time::Duration,
};

pub(super) fn run(
    node: &Node,
    command: &super::Command,
    args: &[OsString],
    within: Duration,
) -> Result<String, String> {
    let (input, owner) = std::io::pipe().map_err(|error| error.to_string())?;
    let mut guard = Command::new(&node.fastctx);
    guard
        .args(["command-guard", "--seconds", &within.as_secs().to_string(), "--"])
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
    // The guard owns the deadline and process group. Losing this write end
    // makes it cancel the native command even when the node was killed.
    let output = fastctx_platform::bounded::output(guard, "Claude retirement", within + Duration::from_secs(3));
    drop(owner);
    let output = output.map_err(|error| error.to_string())?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    if output.status.success() {
        return Ok(stdout);
    }
    Err(format!("Claude retirement ({}): {}{}", output.status, stdout, String::from_utf8_lossy(&output.stderr)))
}
