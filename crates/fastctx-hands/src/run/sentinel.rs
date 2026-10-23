//! Execution boundary for internal observations, inside their dedicated supervisor.
use super::shell::Prepared;
use fastctx_platform::{ProcessTree, tree::TreeOutput};
use fastctx_types::run::{JobRecord, OutputMode};
use std::{ffi::OsStr, io, path::Path};

/// Development builds may run observations without walls, to try sentinels on
/// a machine that has none; released binaries never accept the switch. The
/// node hands the switch on to the supervisors it starts.
pub fn unwalled() -> bool {
    !fastctx_platform::RELEASED && std::env::var_os("FASTCTX_LAB_UNSANDBOXED_SENTINEL").is_some_and(|v| v == "1")
}

/// Why this machine cannot run a sentinel's command, if it cannot.
pub fn unavailable() -> Option<String> {
    if unwalled() { None } else { super::walls::missing() }
}

/// Starts the observation's script behind the walls (`walls.rs`) where the
/// machine has them: read-only, offline, the World's directory hidden but for
/// the script. The supervisor keeps ownership, the deadline, output capture
/// and process-tree cancellation.
pub fn spawn(shell: &Prepared, record: &JobRecord, world: &Path) -> io::Result<(ProcessTree, TreeOutput)> {
    let script = shell.args.last().ok_or_else(|| io::Error::other("observation has no script"))?;
    let shell_args = [OsStr::new("--noprofile"), OsStr::new("--norc"), script.as_os_str()];
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
    let cwd = Path::new(&record.cwd);
    if unwalled() {
        return ProcessTree::spawn_with_io(&shell.program, &shell_args, cwd, &environment, OutputMode::Separate, None);
    }
    // Only what lies inside the hidden World goes back, and never the World
    // whole: a later mount covers an earlier one.
    let kept: Vec<&Path> =
        [Path::new(script), cwd].into_iter().filter(|path| path.starts_with(world) && *path != world).collect();
    let Some((program, mut args)) = super::walls::walls(world, &kept)? else {
        return ProcessTree::spawn_with_io(&shell.program, &shell_args, cwd, &environment, OutputMode::Separate, None);
    };
    args.push(shell.program.clone().into_os_string());
    args.extend(shell_args.iter().map(|arg| arg.to_os_string()));
    let args: Vec<&OsStr> = args.iter().map(|arg| arg.as_os_str()).collect();
    ProcessTree::spawn_with_io(&program, &args, cwd, &environment, OutputMode::Separate, None)
}
