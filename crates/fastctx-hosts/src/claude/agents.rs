//! Non-session Claude Code commands used by the agent picker.

use std::ffi::OsString;
use std::path::Path;

use fastctx_types::agents::{AgentFacts, AgentProgram, Fact, LaunchCheck, ProgramSource};

use crate::agents::{measured, output, timed};

pub fn probe(cwd: &Path, env: &[(OsString, OsString)], path: Option<&OsString>) -> AgentFacts {
    let mut timings = Vec::new();
    let program = measured(&mut timings, "resolve", || super::background::find(path));
    let mut facts = AgentFacts {
        agent: "claude".parse().expect("agent name"),
        program: Fact::known(program.as_ref().map(|path| AgentProgram {
            path: path.clone(),
            source: ProgramSource::Path,
            exists: true,
        })),
        version: Fact::unavailable("no claude command on PATH"),
        launch: Fact::unavailable("no claude command on PATH"),
        login: Fact::unavailable("no claude command on PATH"),
        models: Fact::unavailable(
            "Claude Code has no public non-session CLI model catalog; the World does not use Agent SDK or -p",
        ),
        integration: Fact::unavailable("the node supplies its apply status"),
        timings,
    };
    if let Some(program) = program {
        // These official commands are independent and create no sessions.
        let (version, login, launch) = std::thread::scope(|scope| {
            let version = scope.spawn(|| timed("--version", || crate::agents::version(&program, cwd, env)));
            let login = scope.spawn(|| timed("auth status --json", || crate::agents::login(&program, cwd, env, false)));
            let launch = scope.spawn(|| timed("--help", || launch(&program, cwd, env)));
            (
                version.join().expect("version probe panicked"),
                login.join().expect("login probe panicked"),
                launch.join().expect("launch probe panicked"),
            )
        });
        facts.version = version.0;
        facts.login = login.0;
        facts.launch = launch.0;
        facts.timings.extend([version.1, login.1, launch.1]);
    }
    facts
}

fn launch(program: &Path, cwd: &Path, env: &[(OsString, OsString)]) -> Fact<LaunchCheck> {
    match output(program, &["--help"], cwd, env) {
        Ok(answer) if answer.status.success() => {
            let help = String::from_utf8_lossy(&answer.stdout);
            if help.split_whitespace().any(|word| word.trim_end_matches(',') == "--bg") {
                Fact::known(LaunchCheck::ClaudeCommand)
            } else {
                Fact::failed("this Claude Code does not advertise --bg in --help")
            }
        }
        Ok(answer) => Fact::failed(format!("--help exited {}", answer.status)),
        Err(error) => Fact::failed(error),
    }
}
