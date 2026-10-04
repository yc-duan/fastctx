//! Read-only probes of official commands. Authentication output is reduced
//! to status; account identifiers and even masked keys never leave this layer.

use std::ffi::OsString;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::time::Instant;

use fastctx_types::agents::{Fact, LoginStatus, ProbeTime};

pub fn elapsed(start: Instant) -> u64 {
    u64::try_from(start.elapsed().as_millis()).expect("probe duration fits in milliseconds")
}

pub fn measured<T>(times: &mut Vec<ProbeTime>, step: &str, run: impl FnOnce() -> T) -> T {
    let (value, time) = timed(step, run);
    times.push(time);
    value
}

pub fn timed<T>(step: &str, run: impl FnOnce() -> T) -> (T, ProbeTime) {
    let start = Instant::now();
    let value = run();
    (value, ProbeTime { step: step.into(), elapsed_ms: elapsed(start) })
}

pub fn output(program: &Path, args: &[&str], cwd: &Path, env: &[(OsString, OsString)]) -> Result<Output, String> {
    let mut command = Command::new(program);
    command.args(args).current_dir(cwd).env_clear().envs(env.iter().map(|(k, v)| (k, v)));
    command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        command.creation_flags(0x0800_0000);
    }
    command.output().map_err(|error| format!("running {} {}: {error}", program.display(), args.join(" ")))
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

pub fn login(program: &Path, cwd: &Path, env: &[(OsString, OsString)], codex: bool) -> Fact<LoginStatus> {
    let args: &[&str] = if codex { &["login", "status"] } else { &["auth", "status", "--json"] };
    let answer = match output(program, args, cwd, env) {
        Ok(answer) => answer,
        Err(error) => return Fact::failed(error),
    };
    if codex {
        // Codex 0.160 writes status to stderr. API-key status includes a
        // masked key, so neither full stream is exposed, including on errors.
        let text = format!("{}\n{}", String::from_utf8_lossy(&answer.stdout), String::from_utf8_lossy(&answer.stderr));
        let lines: Vec<_> = text.lines().map(str::trim).collect();
        if answer.status.success() {
            let method = if lines.contains(&"Logged in using ChatGPT") {
                Some("chatgpt")
            } else if lines.iter().any(|line| line.starts_with("Logged in using an API key")) {
                Some("api_key")
            } else {
                None
            };
            // Exit 0 is the documented positive status, including new modes.
            return Fact::known(LoginStatus::LoggedIn { method: method.map(str::to_string) });
        }
        if answer.status.code() == Some(1) && lines.contains(&"Not logged in") {
            return Fact::known(LoginStatus::LoggedOut);
        }
    } else if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&answer.stdout) {
        match (value.get("loggedIn").and_then(serde_json::Value::as_bool), answer.status.code()) {
            (Some(true), Some(0)) => {
                let method = value.get("authMethod").and_then(serde_json::Value::as_str).map(str::to_string);
                return Fact::known(LoginStatus::LoggedIn { method });
            }
            (Some(false), Some(1)) => return Fact::known(LoginStatus::LoggedOut),
            _ => {}
        }
    }
    Fact::failed(format!("{} exited {} without a recognized login status", args.join(" "), answer.status))
}
