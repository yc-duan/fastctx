//! Read-only probes of official commands. Authentication output is reduced
//! to its status and the account's name, organization and plan, which tell a
//! machine's accounts apart; keys, even masked, never leave this layer.

use std::ffi::OsString;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::time::Instant;

use fastctx_types::agents::{AgentAccount, Fact, LoginStatus, ProbeTime};

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

/// Codex's `login status`: whether someone is logged in, and how; Codex
/// names the account only through its app-server (`codex::account`).
pub fn login(program: &Path, cwd: &Path, env: &[(OsString, OsString)]) -> Fact<LoginStatus> {
    let args: &[&str] = &["login", "status"];
    let answer = match output(program, args, cwd, env) {
        Ok(answer) => answer,
        Err(error) => return Fact::failed(error),
    };
    {
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
    }
    Fact::failed(format!("{} exited {} without a recognized login status", args.join(" "), answer.status))
}

/// Claude Code's `auth status --json`, for the home its environment names:
/// whether a login is configured (it does not check that it still works),
/// and, for a claude.ai login, whose. A long-lived token names no one.
pub fn claude_login(
    program: &Path,
    cwd: &Path,
    env: &[(OsString, OsString)],
) -> (Fact<LoginStatus>, Fact<AgentAccount>) {
    let args = ["auth", "status", "--json"];
    let answer = match output(program, &args, cwd, env) {
        Ok(answer) => answer,
        Err(error) => return (Fact::failed(error.clone()), Fact::failed(error)),
    };
    if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&answer.stdout) {
        let text = |key: &str| value.get(key).and_then(serde_json::Value::as_str).map(str::to_string);
        match (value.get("loggedIn").and_then(serde_json::Value::as_bool), answer.status.code()) {
            (Some(true), Some(0)) => {
                let account = AgentAccount {
                    email: text("email"),
                    organization: text("orgName"),
                    plan: text("subscriptionType"),
                };
                return (Fact::known(LoginStatus::LoggedIn { method: text("authMethod") }), Fact::known(account));
            }
            (Some(false), Some(1)) => {
                return (
                    Fact::known(LoginStatus::LoggedOut),
                    Fact::unavailable("no one is logged in to this Claude Code home"),
                );
            }
            _ => {}
        }
    }
    let failed = format!("{} exited {} without a recognized login status", args.join(" "), answer.status);
    (Fact::failed(failed.clone()), Fact::failed(failed))
}

/// Claude Code's `auth status --json` for the home its environment names,
/// as who is logged in and how: `authMethod` tells a `/login` subscription
/// login (`claude.ai`) from a token in the environment (`oauth_token`),
/// which names no account (2.1.294 and 2.1.296). It says a login is
/// configured; it does not check that it still works.
pub fn claude_identity(
    program: &Path,
    cwd: &Path,
    env: &[(OsString, OsString)],
) -> Fact<fastctx_types::logins::Identity> {
    let args = ["auth", "status", "--json"];
    let answer = match output(program, &args, cwd, env) {
        Ok(answer) => answer,
        Err(error) => return Fact::failed(error),
    };
    if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&answer.stdout) {
        let text = |key: &str| value.get(key).and_then(serde_json::Value::as_str).map(str::to_string);
        match (value.get("loggedIn").and_then(serde_json::Value::as_bool), answer.status.code()) {
            (Some(true), Some(0)) => {
                return Fact::known(fastctx_types::logins::Identity {
                    logged_in: true,
                    method: text("authMethod"),
                    email: text("email"),
                    organization: text("orgName"),
                    plan: text("subscriptionType"),
                });
            }
            (Some(false), Some(1)) => return Fact::known(fastctx_types::logins::Identity::default()),
            _ => {}
        }
    }
    Fact::failed(format!("{} exited {} without a recognized login status", args.join(" "), answer.status))
}
