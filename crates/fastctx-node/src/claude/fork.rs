//! A fork is a newly born resident, carrying its own World identity and
//! instructions, in the source resident's town on this machine.

use std::sync::Arc;

use fastctx_hosts::claude::{self, background};
use fastctx_types::ipc::Failure;
use fastctx_types::residents::{About, Host, Origin};
use fastctx_types::session::control::Controlled;
use fastctx_types::{Address, ResidentId};
use jiff::Timestamp;

use crate::Node;

pub async fn fork(node: &Arc<Node>, resident: &ResidentId, through: Option<String>) -> Result<Controlled, Failure> {
    if through.is_some() {
        return Err(Failure::new(
            "Claude Code can fork the saved conversation, but has no public exact-turn fork control",
        ));
    }
    let (source, town) = node.residents.about(resident).ok_or_else(|| Failure::new("unknown resident"))?;
    let session =
        source.session.ok_or_else(|| Failure::new("Claude has not identified the source conversation yet"))?;
    super::history::wait_stored(node, &session).await.map_err(Failure::new)?;
    // A live session may have changed model since it registered. With no
    // live reading, leave the resumed session's model selection to Claude.
    let model = super::control::offers(node, resident).await?.model;
    let id = node.residents.new_id(&source.host.agent)?;
    let super::Command { program, env, .. } = super::command(node)?;
    let (words, heard) = crate::colleagues::instructions(node, &id, town.as_ref(), &Origin::User).await?;
    let directory = node.world.resident(town.as_ref(), &id);
    fastctx_platform::ensure_private_dir(directory.path()).map_err(failure)?;
    let settings = directory.path().join("claude-settings.json");
    let instructions = directory.path().join("claude-instructions.md");
    std::fs::write(&settings, super::settings_json())
        .and_then(|()| std::fs::write(&instructions, words))
        .map_err(failure)?;
    let cwd = crate::colleagues::standing(node, town.as_ref())?;
    let plugin = claude::plugin_dir(node.world.path());
    let world = node.world.path().to_path_buf();
    let source_session = session.clone();
    let selected_model = model.clone();
    let _birth = node.claude.births.read().await;
    let printed = tokio::task::spawn_blocking(move || {
        let start = background::Start {
            plugin: &plugin,
            settings: &settings,
            instructions: &instructions,
            first_words: "",
            model: selected_model.as_deref(),
            reasoning_effort: None,
        };
        super::trust(&world)?;
        background::claude(&program, &background::fork_args(&source_session, &start), &cwd, &env)
    })
    .await
    .map_err(|error| Failure::new(error.to_string()))?
    .map_err(Failure::new)?;
    let short = background::started(&printed).ok_or_else(|| {
        Failure::new(format!("Claude Code did not identify the fork; its output was: {}", printed.trim()))
    })?;
    if session.starts_with(&short) {
        return Err(Failure::new("Claude Code returned the original session for a requested fork"));
    }
    let at = Timestamp::now();
    let about = About {
        id: id.clone(),
        host: Host { agent: source.host.agent, client: "claude-code".into(), version: source.host.version },
        machine: node.machine(),
        born_at: at,
        origin: Origin::User,
        session: Some(short),
        world_started: true,
        model,
        acting: source.acting,
        works_on: source.works_on,
    };
    crate::colleagues::setting::inherit(node, &source.id, &id)?;
    node.residents.create(&node.world, about, town, crate::residents::idle(at)).map_err(failure)?;
    node.residents.set_heard(&id, heard);
    drop(_birth);
    super::replay(node).await;
    Ok(Controlled::Forked { resident: Address { machine: node.machine(), resident: id } })
}

fn failure(error: std::io::Error) -> Failure {
    Failure::new(error.to_string())
}
