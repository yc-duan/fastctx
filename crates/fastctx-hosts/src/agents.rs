//! Read-only runs of an agent's official command, such as `--version`,
//! each bounded in time.

use std::ffi::OsString;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use fastctx_types::agents::Fact;

/// How long a probe may take before it counts as not answering.
const PROBE: std::time::Duration = std::time::Duration::from_secs(30);

pub fn output(program: &Path, args: &[&str], cwd: &Path, env: &[(OsString, OsString)]) -> Result<Output, String> {
    let mut command = Command::new(program);
    command.args(args).current_dir(cwd).env_clear().envs(env.iter().map(|(k, v)| (k, v)));
    command.stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        command.creation_flags(0x0800_0000);
    }
    let what = format!("{} {}", program.display(), args.join(" "));
    fastctx_platform::bounded::output(command, &what, PROBE).map_err(|error| match fastctx_platform::bounded::ran_late(
        &error,
    ) {
        true => format!("{what} did not answer within {} s", PROBE.as_secs()),
        false => format!("running {what}: {error}"),
    })
}

pub fn version(program: &Path, cwd: &Path, env: &[(OsString, OsString)]) -> Fact<String> {
    match output(program, &["--version"], cwd, env) {
        Ok(answer) if answer.status.success() => match String::from_utf8(answer.stdout) {
            Ok(text) if !text.trim().is_empty() => Fact::known(text.trim().to_string()),
            _ => Fact::failed("--version did not return a UTF-8 version"),
        },
        Ok(answer) => Fact::failed(format!("--version exited {}", answer.status)),
        Err(error) => Fact::failed(error),
    }
}
