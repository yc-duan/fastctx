//! Local maintenance remains available when the business IPC is refused.
//! The carried CLI owns migration and its durable state; the shell only
//! transports its typed result and the user's explicit maintenance actions.

use fastctx_types::renewal::Status;

fn run(action: &str) -> Result<Vec<u8>, String> {
    let program = super::setup::carried().ok_or("this build of World carries no maintenance program")?;
    let root = fastctx_platform::fastctx_root().map_err(|error| error.to_string())?;
    let mut command = std::process::Command::new(program);
    command.args(["renew", action, "--dir"]).arg(root);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        command.creation_flags(0x08000000);
    }
    let result = command.output().map_err(|error| error.to_string())?;
    if !result.status.success() {
        return Err(String::from_utf8_lossy(&result.stderr).into_owned());
    }
    Ok(result.stdout)
}

pub fn state() -> Result<Status, String> {
    serde_json::from_slice(&run("status")?).map_err(|error| error.to_string())
}

pub fn now() -> Result<(), String> {
    run("now").map(|_| ())
}

