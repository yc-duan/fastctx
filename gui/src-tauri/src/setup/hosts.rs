//! Host changes run in the selected World kernel, even from an older GUI.

use std::path::Path;
use std::process::{Command, Stdio};

use fastctx_install::apply::{self, Places};

pub(super) fn run(
    places: &Places,
    codex: bool,
    claude: bool,
    raise: bool,
    into: bool,
    home: Option<&Path>,
) -> Result<Vec<String>, String> {
    if !codex && !claude {
        return Ok(Vec::new());
    }
    let program =
        if into { apply::place_executable(places).map_err(|error| error.to_string())?.0 } else { places.installed() };
    if !into && fastctx_install::program::inspect(&program).map_err(|error| error.to_string())?.protocol == 0 {
        let report = apply::unapply(places, apply::Hosts { codex, claude }).map_err(|error| error.to_string())?;
        return Ok(report
            .notices
            .iter()
            .map(|notice| format!("{}: {}", notice.location, notice.reason))
            .chain(report.leftovers)
            .collect());
    }
    let mut command = Command::new(program);
    command.arg(if into { "apply" } else { "unapply" }).env("FASTCTX_HOME", &places.root).stdin(Stdio::null());
    for (chosen, target) in [(codex, "codex"), (claude, "claude")] {
        if chosen {
            command.args(["--target", target]);
            if let Some(home) = home {
                command.arg("--home").arg(home);
            }
        }
    }
    if into && raise {
        command.arg("--raise-output-limit");
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        command.creation_flags(0x08000000);
    }
    let output = command.output().map_err(|error| error.to_string())?;
    let stdout = String::from_utf8(output.stdout).map_err(|error| error.to_string())?;
    let stderr = String::from_utf8(output.stderr).map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(format!("{stdout}{stderr}"));
    }
    // Only what stayed as it was reaches the page: the rest of the report
    // (`say` in `crates/fastctx/src/apply.rs`) is for the terminal and the log.
    Ok(stdout
        .lines()
        .chain(stderr.lines())
        .filter_map(|line| line.strip_prefix("left ").or_else(|| line.strip_prefix("could not clear: ")))
        .map(str::to_owned)
        .collect())
}
