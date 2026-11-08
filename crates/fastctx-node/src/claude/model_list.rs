//! Claude conversations go on with the models their login offers
//! (`world-gui.md`, 对话): when those who hold the login take out the model
//! a conversation was on, it moves to the one the login starts on. The
//! choice is kept where a turn reads it as it starts (the session's
//! `selection.json`, and the options it is started again with), so a
//! running conversation takes it from its next turn and a sleeping one is
//! not woken for it.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use fastctx_hosts::claude::control as files;
use fastctx_types::ResidentId;
use fastctx_types::access::Acting;
use fastctx_types::ipc::Failure;
use serde_json::{Value, json};

use crate::Node;
use crate::logins::models::Offering;

/// Moves the conversations of the Claude Code login `dir` here off the
/// models it no longer offers, in the background.
pub(crate) fn follow(node: &Arc<Node>, dir: PathBuf) {
    let node = node.clone();
    fastctx_platform::task::spawn(async move {
        if let Err(error) = settle(&node, &dir).await {
            node.log.line(format_args!(
                "moving the conversations of {} off models it no longer offers: {}",
                dir.display(),
                error.message
            ));
        }
    });
}

async fn settle(node: &Arc<Node>, dir: &Path) -> Result<(), Failure> {
    let offering = Offering::of(node, Some(dir))?;
    let default = crate::logins::Context::of(node)?.claude_default;
    let conversations: Vec<(ResidentId, Option<String>, Acting)> = node
        .residents
        .everyone()
        .filter(|one| one.removed.is_none() && one.about.host.agent.as_str() == "claude")
        .filter(|one| crate::homes::same_dir(one.about.home.as_deref().unwrap_or(&default), dir))
        .map(|one| (one.id, one.about.session, one.about.acting))
        .collect();
    for (id, session, acting) in conversations {
        if let Err(error) = settle_one(node, &offering, &id, session.as_deref(), &acting).await {
            node.log.line(format_args!("moving {id} off a model its login no longer offers: {}", error.message));
        }
    }
    Ok(())
}

async fn settle_one(
    node: &Arc<Node>,
    offering: &Offering,
    id: &ResidentId,
    session: Option<&str>,
    acting: &Acting,
) -> Result<(), Failure> {
    let failure = |error: std::io::Error| Failure::new(error.to_string());
    let directory = session.map(|session| files::directory(node.world.path(), session));
    let selected = match &directory {
        Some(directory) => files::read::<Value>(&directory.join("selection.json")).map_err(failure)?,
        None => None,
    };
    let initial = crate::session_options::load(node, id).await?;
    let current = selected.as_ref().and_then(|value| value["model"].as_str()).map(str::to_owned).or(initial.model);
    let Some(current) = current else { return Ok(()) };
    if offering.offers(&current) {
        return Ok(());
    }
    let profile = crate::profile::session::get(node, acting);
    let next = offering.starting(profile.claude.get("model").and_then(Value::as_str))?;
    if let Some(directory) = directory.filter(|directory| fastctx_platform::disk::is_dir(directory)) {
        // As choosing a model there does: the effort named for the old one
        // and the engine's own route are dropped.
        let mut selection = selected.filter(Value::is_object).unwrap_or_else(|| json!({}));
        selection["model"] = next.clone().into();
        if let Some(fields) = selection.as_object_mut() {
            fields.remove("effort");
        }
        let bytes = serde_json::to_vec(&selection).map_err(|error| Failure::new(error.to_string()))?;
        fastctx_platform::publish(&directory.join("selection.json"), &bytes, fastctx_platform::Permissions::Private)
            .map_err(failure)?;
        fastctx_platform::publish(&directory.join("routing.json"), b"null", fastctx_platform::Permissions::Private)
            .map_err(failure)?;
    }
    crate::session_options::set_model(node, id, next.clone(), None).await?;
    node.log.line(format_args!("{id} moves from {current}, which its login no longer offers, to {next}"));
    Ok(())
}
