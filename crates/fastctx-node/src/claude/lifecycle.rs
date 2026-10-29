//! Claude's lifecycle adapter. Only the supervisor's CLI stops a worker.

use crate::Node;
use fastctx_hosts::claude::{background, lifecycle::IdleWorker};
use fastctx_types::{ipc::Failure, residents::About};
use std::{collections::HashMap, ffi::OsString, path::PathBuf, sync::Arc, time::Duration};

// The retirement gate must be released well before an external caller's
// thirty-second admission deadline, even if Claude Code stops responding.
const PROBE: Duration = Duration::from_secs(3);
const STOP: Duration = Duration::from_secs(5);

pub(crate) async fn idle(node: &Arc<Node>, about: &About) -> Result<Option<IdleWorker>, Failure> {
    if !node.residents.host_present(&about.id) {
        return Ok(None);
    }
    let Some(session) = &about.session else { return Ok(None) };
    let (command, home) = command_home(node, about)?;
    let listed = list(command, PROBE).await?;
    fastctx_hosts::claude::lifecycle::idle_worker(&home, session, &listed).map_err(Failure::new)
}

/// Whether Claude Code lists the resident's session waiting for its next
/// prompt with nothing of its own in flight.
pub(crate) async fn resting(node: &Arc<Node>, about: &About) -> Result<bool, Failure> {
    let Some(session) = &about.session else { return Ok(false) };
    let (command, home) = command_home(node, about)?;
    let listed = list(command, Duration::from_secs(30)).await?;
    fastctx_hosts::claude::lifecycle::resting(&home, session, &listed).map_err(Failure::new)
}

fn command_home(node: &Node, about: &About) -> Result<(super::Command, PathBuf), Failure> {
    let command = super::command(node, about.home.as_deref())?;
    let home = command
        .env
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case("CLAUDE_CONFIG_DIR"))
        .map(|(_, value)| std::path::PathBuf::from(value))
        .map(Ok)
        .unwrap_or_else(|| fastctx_platform::user_home().map(|home| home.join(".claude")))
        .map_err(|error| Failure::new(error.to_string()))?;
    Ok((command, home))
}

async fn list(command: super::Command, timeout: Duration) -> Result<String, Failure> {
    tokio::task::spawn_blocking(fastctx_platform::doing::carried(move || {
        background::invoke(
            &command.program,
            &["agents".into(), "--json".into(), "--all".into()],
            &command.cwd,
            &command.env,
            timeout,
        )
        .map_err(|error| error.message)
    }))
    .await
    .map_err(|error| Failure::new(error.to_string()))?
    .map_err(Failure::new)
}

/// A scan owns its listings; failed listings are shared too, never retried per session.
#[derive(Default)]
pub(crate) struct Scan(HashMap<PathBuf, Result<String, Failure>>);

impl Scan {
    pub(crate) async fn idle(&mut self, node: &Arc<Node>, about: &About) -> Result<Option<IdleWorker>, Failure> {
        let Some(session) = &about.session else { return Ok(None) };
        let (command, home) = command_home(node, about)?;
        if !self.0.contains_key(&home) {
            let listed = list(command, Duration::from_secs(30)).await;
            self.0.insert(home.clone(), listed);
        }
        let listed = self.0.get(&home).expect("the login was listed").as_ref().map_err(Clone::clone)?;
        fastctx_hosts::claude::lifecycle::idle_worker(&home, session, listed).map_err(Failure::new)
    }
}

/// The caller has rechecked all World and host evidence under both gates.
pub(crate) async fn collect(node: &Arc<Node>, about: &About, worker: &IdleWorker) -> Result<bool, Failure> {
    let command = super::command(node, about.home.as_deref())?;
    let id = worker.id.clone();
    let session = about.session.clone().ok_or_else(|| Failure::new("the host session is unknown"))?;
    let stopping_node = node.clone();
    let running = tokio::task::spawn_blocking(fastctx_platform::doing::carried(move || {
        let args: Vec<OsString> = vec!["stop".into(), id.into()];
        super::retire_command::run(&stopping_node, &command, &args, STOP)?;
        let listed = background::invoke(
            &command.program,
            &["agents".into(), "--json".into(), "--all".into()],
            &command.cwd,
            &command.env,
            PROBE,
        )
        .map_err(|error| error.message)?;
        background::running(&listed, &session)
    }))
    .await
    .map_err(|error| Failure::new(error.to_string()))?
    .map_err(Failure::new)?;
    if running {
        return Err(Failure::new("Claude Code accepted retirement but still reports a running worker"));
    }
    // An idle retirement has no turn or delivery to settle. The ordinary
    // report observer still consumes any final receipts from this activation.
    Ok(true)
}
