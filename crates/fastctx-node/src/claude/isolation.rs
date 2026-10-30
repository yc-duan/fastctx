//! An older assisted session must load isolation at process creation. Editing
//! settings of a live Claude worker does not unload its instructions or tools.

use crate::Node;
use fastctx_hosts::claude::{background, control};
use fastctx_types::{ResidentId, ipc::Failure, residents::Activity};
use std::{path::PathBuf, sync::Arc};

fn path(node: &Node, id: &ResidentId) -> PathBuf {
    node.world.conversation(id).path().join("host-isolation-source.json")
}

/// Keeps the original session before enabling isolation or touching its host.
pub(crate) fn begin(node: &Node, id: &ResidentId, session: &str) -> Result<(), Failure> {
    control::write(&path(node, id), &session).map_err(|error| Failure::new(error.to_string()))
}

/// A recorded new session proves that continuation already happened, including
/// when the node stopped before deleting the migration intent.
pub(super) fn source(node: &Node, id: &ResidentId) -> Result<Option<String>, Failure> {
    if crate::assists::host::kept(node, id)? != Some(true) {
        return Ok(None);
    }
    let Some(session) = node.residents.about(id).and_then(|(about, _)| about.session) else { return Ok(None) };
    let held = control::read::<String>(&path(node, id)).map_err(|error| Failure::new(error.to_string()))?;
    if let Some(source) = held {
        let same = source == session
            || (session.len() == 8 && source.starts_with(&session))
            || (source.len() == 8 && session.starts_with(&source));
        if !same {
            finished(node, id)?;
            return Ok(None);
        }
        return Ok(Some(if source.len() == 8 { session } else { source }));
    }
    let host = control::host(&control::directory(node.world.path(), &session))
        .map_err(|error| Failure::new(error.to_string()))?;
    // A host that does not say it was started isolated was not: those
    // registered before the flag was reported (2026-10-21) say nothing.
    if host.is_some_and(|host| host.assisted != Some(true)) {
        begin(node, id, &session)?;
        return Ok(Some(session));
    }
    Ok(None)
}

fn finished(node: &Node, id: &ResidentId) -> Result<(), Failure> {
    match std::fs::remove_file(path(node, id)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(Failure::new(format!("finishing host isolation: {error}"))),
    }
}

/// The caller owns this conversation's action gate. An old worker retires only
/// after both the World and Claude report it idle; its current turn is preserved.
pub(super) fn prepare<'a>(
    node: &'a Arc<Node>,
    id: &'a ResidentId,
    alive: bool,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<bool, Failure>> + Send + 'a>> {
    Box::pin(async move {
        let Some(source) = source(node, id)? else { return Ok(alive) };
        if !alive {
            return Ok(false);
        }
        let (about, _) = node.residents.about(id).ok_or_else(crate::authorization::no_subject)?;
        if node.residents.now(id).is_some_and(|now| now.turn.is_some() || now.activity == Activity::Busy)
            || !super::lifecycle::resting(node, &about).await?
        {
            return Err(Failure::host_unavailable("host isolation waits for the current Claude work to finish"));
        }
        super::management::stop(node, about.home.as_deref(), id, &source, true).await?;
        Ok(false)
    })
}

/// Continues through the existing history-preserving path with all isolation
/// startup flags, rather than the older supervisor's saved startup options.
pub(super) async fn continue_in(
    node: &Arc<Node>,
    id: &ResidentId,
    source: &str,
    words: &str,
) -> Result<String, background::CommandFailure> {
    let (about, _) = node
        .residents
        .about(id)
        .ok_or_else(|| background::CommandFailure { message: "the conversation was deleted".into(), started: false })?;
    let command = super::command(node, about.home.as_deref())
        .map_err(|error| background::CommandFailure { message: error.message, started: false })?;
    super::oldest::check(node, &command)
        .await
        .map_err(|error| background::CommandFailure { message: error.message, started: false })?;
    let short = super::continuing::start(
        node,
        id,
        about.home.as_deref(),
        source,
        words,
        "its original host predates isolation",
    )
    .await?;
    finished(node, id).map_err(|error| background::CommandFailure { message: error.message, started: true })?;
    Ok(short)
}
