//! Execution boundary for internal observations, inside their dedicated supervisor.
use super::shell::Prepared;
use fastctx_platform::{ProcessTree, tree::TreeOutput};
use fastctx_types::run::{JobRecord, OutputMode};
use std::{ffi::OsStr, io, path::Path};

/// Development-only execution until the platform's offline read-only sandbox is
/// connected here. Released binaries never accept the laboratory switch.
pub fn available() -> bool {
    !fastctx_platform::RELEASED && std::env::var_os("FASTCTX_LAB_UNSANDBOXED_SENTINEL").is_some_and(|v| v == "1")
}
/// The sandbox implementation replaces this function; the supervisor retains
/// ownership, a 30-second deadline, output capture and process-tree cancellation.
pub fn spawn(shell: &Prepared, record: &JobRecord) -> io::Result<(ProcessTree, TreeOutput)> {
    if !available() {
        return Err(io::Error::other("the offline read-only sentinel sandbox is not connected"));
    }
    let script = shell.args.last().ok_or_else(|| io::Error::other("observation has no script"))?;
    let args = [OsStr::new("--noprofile"), OsStr::new("--norc"), script.as_os_str()];
    let environment: Vec<_> = shell
        .environment
        .iter()
        .filter(|(key, _)| {
            let key = key.to_string_lossy();
            !key.starts_with("CLAUDE")
                && !key.starts_with("CODEX")
                && !key.starts_with("BASH_FUNC_")
                && key != "BASH_ENV"
                && (!key.starts_with("FASTCTX_")
                    || matches!(key.as_ref(), "FASTCTX_HOME" | "FASTCTX_BIN" | "FASTCTX_WRITES"))
        })
        .cloned()
        .collect();
    ProcessTree::spawn_with_io(&shell.program, &args, Path::new(&record.cwd), &environment, OutputMode::Separate, None)
}
