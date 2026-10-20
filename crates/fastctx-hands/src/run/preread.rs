//! Read the complete source before reserving a job or executing any of it.

use std::io::Write;
use std::process::{Command, Stdio};

use fastctx_types::ipc::Failure;

/// The machine names written literally after `on` in `script`, each with
/// its line, for checking before anything runs.
pub fn machines_named(script: &str) -> Vec<(String, usize)> {
    super::heredoc::on_machines(script)
}

/// A script containing only literal `on` commands. The first machine can
/// orchestrate the whole script; a single command also supplies its body.
/// No expansion, local filesystem access or local bash is involved in
/// recognizing it: the machine that runs what is sent checks it there.
pub fn on_only(script: &str) -> Result<(String, Option<String>), Failure> {
    super::heredoc::on_only(script)
}

/// Checks source with the machine's own bash and checks here-document hazards.
/// No startup files or source commands are executed. This checks literal source,
/// not programs generated at runtime by `eval`, `source`, or another interpreter.
pub fn check(script: &str) -> Result<(), Failure> {
    if let Some(offset) = script.find('\0') {
        return Err(super::heredoc::refuse(
            script[..offset].bytes().filter(|&byte| byte == b'\n').count() + 1,
            "NUL bytes are not bash source; remove the NUL or encode the data outside the script",
        ));
    }
    let bash = super::shell::bash().map_err(|error| failure("could not locate bash", error))?;
    let mut syntax = Command::new(bash);
    syntax
        .args(["--noprofile", "--norc", "-n", "-O", "extglob", "-s"])
        // Imported options, functions and BASH_ENV must not affect a source check.
        .env_clear()
        .env("LC_ALL", "C")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // The parser uses pipes only; allocating a console would consume
        // desktop resources for every source check in a headless node.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        syntax.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = syntax.spawn().map_err(|error| failure("could not start bash's syntax check", error))?;
    let mut input = child.stdin.take().expect("piped stdin");
    // Bash can report a syntax error before consuming all input. Drain its
    // diagnostics while feeding source so neither pipe can block the other.
    let (output, written) = std::thread::scope(|scope| {
        let writer = scope.spawn(move || input.write_all(script.as_bytes()));
        let output = child.wait_with_output();
        (output, writer.join().expect("source writer panicked"))
    });
    let output = output.map_err(|error| failure("could not collect bash's syntax check", error))?;
    if !output.status.success() || !output.stderr.is_empty() || !output.stdout.is_empty() {
        let stderr = String::from_utf8(output.stderr)
            .map_err(|error| failure("bash returned non-UTF-8 syntax diagnostics", error))?;
        let stdout = String::from_utf8(output.stdout)
            .map_err(|error| failure("bash returned non-UTF-8 syntax output", error))?;
        if stderr.is_empty() && stdout.is_empty() {
            return Err(Failure::new(format!(
                "bash's syntax check ended without saying why ({}), so the script could not be checked; simplify nested expressions or split the script.",
                output.status,
            )));
        }
        return Err(Failure::new(format!(
            "bash could not read the whole script:\n{}",
            format!("{stderr}{stdout}").trim_end(),
        )));
    }
    written.map_err(|error| failure("could not send the complete script to bash", error))?;
    super::heredoc::check(script)
}

fn failure(what: &str, error: impl std::fmt::Display) -> Failure {
    Failure::new(format!("{what}: {error}"))
}
