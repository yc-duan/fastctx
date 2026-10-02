//! `run` on this machine: the World's shell (`specs/world-run.md`) for the resident that calls
//! it.
//!
//! Each call is a fresh bash in the resident's host environment, plus the resident's name and
//! town so that `fastctx w`, `fastctx view` and `fastctx project` know who is asking. The run
//! engine waits up to 15 seconds; a command still running then goes on in the background with
//! its record and output in the resident's directory on this machine, and its end hook tells the
//! node, which tells the resident.

use crate::run::{ContentBlock, RunRequest, RunResult};
use crate::session::SessionEnvironment;
use crate::town::core::Town;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

/// Who is running a command.
pub(crate) struct Runner {
    pub(crate) town: Arc<Town>,
    pub(crate) resident: String,
}

/// Runs one command on this machine and returns the engine's result.
pub(crate) async fn run(
    runner: &Runner,
    environment: &SessionEnvironment,
    command: String,
    cwd: Option<String>,
) -> Result<RunResult, String> {
    let town = &runner.town;
    let id = &runner.resident;
    let cwd = match cwd.filter(|cwd| !cwd.trim().is_empty()) {
        Some(cwd) if PathBuf::from(&cwd).is_absolute() => PathBuf::from(cwd),
        Some(cwd) => return Err(format!("cwd must be an absolute directory; got {cwd}.")),
        None => town.home().to_path_buf(),
    };
    let place = town.town_of(id).flatten();
    let mut env: BTreeMap<String, String> = environment
        .variables()
        .iter()
        .filter_map(|(name, value)| Some((name.to_str()?.to_string(), value.to_str()?.to_string())))
        .collect();
    env.insert(crate::town::session::RESIDENT_VARIABLE.to_string(), id.clone());
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
    let outcome = match &result {
        Ok(result) => match result.job.exit_code {
            Some(code) => format!("exit {code}"),
            None => format!("{} {}", format!("{:?}", result.job.state).to_lowercase(), result.job.id),
        },
        Err(error) => format!("failed: {}", error.lines().next().unwrap_or_default()),
    };
    let first = command.lines().find(|line| !line.trim().is_empty()).unwrap_or_default().trim();
    town.record_action(id, town.machine(), "run", first, &outcome);
    // A command handed back while it still runs owes its starter the news of its end.
    if let Ok(result) = &result
        && result.job.exit_code.is_none()
        && let Err(error) = town.owe_end(&result.record_path).await
    {
        town.log(format!("cannot promise {id} the end of {}: {error}", result.job.id));
    }
    result
}

/// The engine's blocks as MCP content: text and images in the order they were printed.
pub(crate) fn content(result: &RunResult) -> Vec<rmcp::model::ContentBlock> {
    crate::run::render(result)
        .iter()
        .map(|block| match block {
            ContentBlock::Text { text } => rmcp::model::ContentBlock::text(text.clone()),
            ContentBlock::Image { data, mime_type, .. } => rmcp::model::ContentBlock::image(data.clone(), mime_type.clone()),
        })
        .collect()
}
