//! `run` on this machine: the World's shell (`specs/world-run.md`) for the resident that calls
//! it, whether it lives here or reached this machine from another one.
//!
//! Each call is a fresh bash with the resident's name and town, so that `fastctx w`, `fastctx
//! view` and `fastctx project` know who is asking. The run engine waits up to 15 seconds; a
//! command still running then goes on in the background with its record and output in the
//! resident's directory on this machine, and its end hook tells the node, which tells the
//! resident: directly when it lives here, by mail when it lives elsewhere.

use crate::model::{ToolContent, ToolResponse};
use crate::run::{ContentBlock, RunRequest, RunResult};
use crate::session::SessionEnvironment;
use super::Starter;
use crate::town::core::Town;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

/// Who is running a command, on this machine.
pub(crate) struct Runner {
    pub(crate) town: Arc<Town>,
    pub(crate) starter: Starter,
}

/// Runs one command on this machine and returns the engine's result.
pub(crate) async fn run(
    runner: &Runner,
    environment: &SessionEnvironment,
    command: String,
    cwd: Option<String>,
) -> Result<RunResult, String> {
    let town = &runner.town;
    let id = runner.starter.id();
    let cwd = match cwd.filter(|cwd| !cwd.trim().is_empty()) {
        Some(cwd) if PathBuf::from(&cwd).is_absolute() => PathBuf::from(cwd),
        Some(cwd) => return Err(format!("cwd must be an absolute directory; got {cwd}.")),
        None => town.home().to_path_buf(),
    };
    let place = match &runner.starter {
        Starter::Here(id) => town.town_of(id).flatten(),
        Starter::Guest { town: place, .. } => place.clone(),
    };
    let mut env: BTreeMap<String, String> = environment
        .variables()
        .iter()
        .filter_map(|(name, value)| Some((name.to_str()?.to_string(), value.to_str()?.to_string())))
        .collect();
    env.insert(crate::town::session::RESIDENT_VARIABLE.to_string(), id.to_string());
    match &place {
        Some(place) => env.insert(crate::town::cli::TOWN_VARIABLE.to_string(), place.clone()),
        None => env.remove(crate::town::cli::TOWN_VARIABLE),
    };
    let fastctx = std::env::current_exe().map_err(|error| format!("Cannot locate the running fastctx binary: {error}"))?;
    let request = RunRequest {
        command: command.clone(),
        cwd,
        env,
        directory: town.resident_dir(place.as_deref(), id),
        hook: Some(format!("'{}' job-ended", fastctx.to_string_lossy().replace('\\', "/"))),
        machine: town.machine().to_string(),
    };
    let result = tokio::task::spawn_blocking(move || crate::run::execute(request))
        .await
        .map_err(|error| format!("The run engine stopped: {error}"))?;
    if let Starter::Here(id) = &runner.starter {
        let (what, outcome) = action(&command, &result);
        town.record_action(id, town.machine(), "run", &what, &outcome);
    }
    // A command handed back while it still runs owes its starter the news of its end.
    let home = match &runner.starter {
        Starter::Here(_) => None,
        Starter::Guest { peer, .. } => Some(peer.machine.as_str()),
    };
    if let Ok(result) = &result
        && result.job.exit_code.is_none()
        && let Err(error) = town.owe_end(&result.record_path, home).await
    {
        town.log(format!("cannot promise {id} the end of {}: {error}", result.job.id));
    }
    result
}

/// What an action line says about a run: its command's first line, and how it went.
pub(crate) fn action(command: &str, result: &Result<RunResult, String>) -> (String, String) {
    let outcome = match result {
        Ok(result) => match result.job.exit_code {
            Some(code) => format!("exit {code}"),
            None => format!("{} {}", format!("{:?}", result.job.state).to_lowercase(), result.job.id),
        },
        Err(error) => format!("failed: {}", error.lines().next().unwrap_or_default()),
    };
    let first = command.lines().find(|line| !line.trim().is_empty()).unwrap_or_default().trim();
    (first.to_string(), outcome)
}

/// The engine's blocks as a tool response: text and images in the order they were printed.
pub(crate) fn response(result: &RunResult) -> ToolResponse {
    let content = crate::run::render(result)
        .iter()
        .map(|block| match block {
            ContentBlock::Text { text } => ToolContent::Text(text.clone()),
            ContentBlock::Image { data, mime_type, .. } => {
                ToolContent::Image { data: data.clone(), mime_type: mime_type.clone(), detail: None }
            }
        })
        .collect();
    ToolResponse { content, is_error: false }
}
