//! A fork is a newly born resident, carrying its own World identity and
//! instructions, in the source resident's town on this machine.

use std::sync::Arc;

use fastctx_hosts::claude::{self, background};
use fastctx_types::ipc::Failure;
use fastctx_types::residents::{About, Host, Origin};
use fastctx_types::session::control::Controlled;
use fastctx_types::{Address, ResidentId};
use jiff::Timestamp;
use serde_json::json;

use crate::Node;

pub async fn fork(node: &Arc<Node>, resident: &ResidentId, through: Option<String>) -> Result<Controlled, Failure> {
    if let Some(why) = super::fork_ready::pending(node, resident)? {
        return Err(Failure::new(why));
    }
    // The action gate keeps source mutations ordered. Host replies must be
    // consumed before taking the birth gate: their reader also registers sessions.
    node.residents.active(resident)?;
    let (source, town) = node.residents.about(resident).ok_or_else(|| Failure::new("unknown resident"))?;
    if let Some(town) = &town {
        crate::town::access::ready(node, &source.acting, town).await?;
    }
    let session =
        source.session.ok_or_else(|| Failure::new("Claude has not identified the source conversation yet"))?;
    super::history::wait_stored(node, &session).await.map_err(Failure::new)?;
    // Saved request choices, rather than a fallback's resolved model, belong
    // to the fork. The same selection is passed through the official CLI.
    let running = super::control::running(node, resident)?;
    let selection = super::control::selection(node, resident, running).await?;
    let model = selection.model;
    let effort = selection.reasoning_effort;
    let mut checkpoints = if running {
        Some(super::control::invoke(node, resident, json!({"do":"checkpoint_export"})).await?)
    } else {
        let path = fastctx_hosts::claude::control::directory(node.world.path(), &session).join("checkpoints.json");
        match fastctx_hosts::claude::control::read_message(&path) {
            Ok(value) => Some(value),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(failure(error)),
        }
    }
    .filter(|value| value["full"] == true);
    let checkpoint = match &through {
        Some(turn) if running => {
            Some(super::control::invoke(node, resident, json!({"do":"checkpoint", "through":turn})).await?)
        }
        Some(turn) => {
            let checkpoints = checkpoints
                .as_ref()
                .ok_or_else(|| Failure::new("this session has no complete saved message prefix"))?;
            let point = &checkpoints["turns"][turn];
            let count: usize = point["after"]
                .as_u64()
                .ok_or_else(|| Failure::new("this turn has no saved message checkpoint"))?
                .try_into()
                .map_err(|_| Failure::new("Claude checkpoint exceeds this platform"))?;
            let hashes = checkpoints["messages"]
                .as_array()
                .filter(|hashes| count > 0 && count <= hashes.len())
                .ok_or_else(|| Failure::new("this turn has no retained message prefix"))?;
            Some(json!({"hashes": &hashes[..count], "retained_until":point["after_uuid"]}))
        }
        None => None,
    };
    let inherited = super::fork_history::retained(node, resident, &session, through.as_deref())?;
    let id = node.residents.new_id(&source.host.agent)?;
    let launching = super::command(node, source.home.as_deref())?;
    super::oldest::check(node, &launching).await?;
    let super::Command { program, env, .. } = launching;
    let (words, heard) =
        crate::colleagues::instructions(node, &id, town.as_ref(), &Origin::User, &source.acting, false).await?;
    let directory = node.world.conversation(&id);
    crate::focus::open(node, &directory, &source.acting, &source.focus).await?;
    super::fork_ready::begin(node, &id)?;
    let cwd = crate::colleagues::standing(node, town.as_ref())?;
    crate::profile::session::ready(node, &source.acting).await?;
    let assisting = crate::assists::host::starting(node, &id, &source.acting)?;
    let at = Timestamp::now();
    let about = About {
        forked_from: Some(Address { machine: node.machine(), resident: resident.clone() }),
        mcp_off: source.mcp_off.clone(),
        id: id.clone(),
        host: Host { agent: source.host.agent, client: "claude-code".into(), version: source.host.version },
        machine: node.machine(),
        born_at: at,
        origin: Origin::User,
        session: None,
        world_started: true,
            native_resumed: false,
        model: model.clone(),
        acting: source.acting.clone(),
        focus: source.focus,
        home: source.home,
        callee: None,
    };
    let mut now = crate::residents::idle(at);
    now.activity = fastctx_types::residents::Activity::Busy;
    if let Some(town) = &town {
        crate::town::access::check(node, &about.acting, town)?;
    }
    node.residents.create(&node.world, about, town.clone(), now).map_err(failure)?;

    let completion = async {
        crate::context::instructed(node, &id, &heard).map_err(|error| Failure::new(error.to_string()))?;
        node.residents.set_instructed(&id, heard);
        crate::session_options::keep(
            node,
            &id,
            &fastctx_types::agents::SessionOptions {
                model: model.clone(),
                reasoning_effort: effort.clone(),
                mcp_off: source.mcp_off.clone(),
            },
        )?;
        let settings = directory.claude_settings();
        let instructions = directory.claude_instructions();
        std::fs::write(&settings, super::settings_json(&id))
            .and_then(|()| std::fs::write(&instructions, words))
            .map_err(failure)?;
        let mcp = crate::profile::session::claude(node, &id, true)?;
        let plugin = claude::plugin_dir(node.world.path());
        let world = node.world.path().to_path_buf();
        let snapshot = checkpoint
            .as_ref()
            .map(|point| super::fork_source::prefix(node, &session, directory.path(), point, &mut checkpoints))
            .transpose()?;
        let source_session = match &snapshot {
            Some(path) => {
                path.to_str().ok_or_else(|| Failure::new("the fork source path is not valid Unicode"))?.to_owned()
            }
            None => session.clone(),
        };
        let expected_attachments = super::fork_attachments::expected(node, &session, snapshot.as_deref()).await?;
        let selected_model = model.clone();
        let selected_effort = effort.clone();

        let birth = node.claude.births.read().await;
        let gate = node.residents.management(resident)?;
        let source_guard = crate::gate_waits::shared(node, resident, &gate).await?;
        node.residents.active(resident)?;
        if let Some(town) = &town {
            crate::town::access::check(node, &source.acting, town)?;
        }
        let printed = tokio::task::spawn_blocking(fastctx_platform::doing::carried(move || {
            let start = background::Start {
                plugin: &plugin,
                settings: &settings,
                mcp: &mcp,
                instructions: &instructions,
                first_words: "",
                model: selected_model.as_deref(),
                reasoning_effort: selected_effort.as_deref(),
                assisting,
            };
            super::trust(&[world.as_path(), cwd.as_path()], &env)?;
            background::claude(&program, &background::fork_args(&source_session, &start), &cwd, &env)
        }))
        .await
        .map_err(|error| Failure::new(error.to_string()))?
        .map_err(Failure::new)?;
        let short = background::started(&printed).ok_or_else(|| {
            Failure::new(format!("Claude Code did not identify the fork; its output was: {}", printed.trim()))
        })?;
        if session.starts_with(&short) {
            return Err(Failure::new("Claude Code returned the original session for a requested fork"));
        }
        node.residents.set_session(&node.world, &id, &short, model, None).map_err(failure)?;

        let finishing = async {
            drop(birth);
            super::fork_ready::wait(node, &id, false).await?;
            super::control::invoke(node, &id, json!({"do":"fork_prepare"})).await?;
            super::fork_ready::wait(node, &id, true).await?;
            super::fork_history::seed(node, &id, &inherited)?;
            crate::turns::apply(
                node,
                &id,
                fastctx_types::session::SessionEvent {
                    at,
                    happened: fastctx_types::session::Happened::Changed {
                        turn: None,
                        change: fastctx_types::session::SessionChange::Forked {
                            source: Address { machine: node.machine(), resident: resident.clone() },
                            thread: session,
                            through: through.clone(),
                        },
                    },
                },
            )
            .await
            .map_err(failure)?;
            if let Some(checkpoints) = checkpoints {
                let imported =
                    super::control::invoke(node, &id, json!({"do":"checkpoint_import", "checkpoints":checkpoints}))
                        .await;
                imported?;
            }
            super::fork_attachments::inherit(node, resident, &id, &expected_attachments).await?;
            if let Some(snapshot) = snapshot {
                std::fs::remove_file(&snapshot).map_err(failure)?;
                std::fs::remove_dir(snapshot.parent().expect("fork source directory")).map_err(failure)?;
            }
            Ok::<_, Failure>(())
        }
        .await;
        if let Err(error) = finishing {
            super::fork_ready::failed(node, &id, &error.message)?;
            return Ok(Controlled::ForkIncomplete {
                resident: Address { machine: node.machine(), resident: id.clone() },
                why: error.message,
            });
        }
        drop(source_guard);
        super::fork_ready::finish(node, &id).await?;
        crate::focus::forked(node, &id).await?;
        Ok(Controlled::Forked { resident: Address { machine: node.machine(), resident: id.clone() } })
    }
    .await;
    if let Err(error) = &completion {
        super::fork_ready::failed(node, &id, &error.message)?;
    }
    completion
}

fn failure(error: std::io::Error) -> Failure {
    Failure::new(error.to_string())
}
