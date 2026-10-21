//! User management follows the resident's machine. Host effects and World
//! membership are reported separately so partial completion stays visible.
//! Archiving keeps everything of a conversation and takes it out of the
//! list; deleting it is `crate::forget`'s.

use crate::{Node, speech};
use fastctx_types::access::Written;
use fastctx_types::access::{Acting, Purpose};
use fastctx_types::ipc::Failure;
use fastctx_types::management::{Managed, Management, ManagementOutcome as Outcome, ManagementStep};
use fastctx_types::remote::{RemoteCall, RemoteReply};
use fastctx_types::residents::{Activity, Delivery, Notice, WorldFact};
use fastctx_types::{Address, ResidentId, TownName};
use jiff::Timestamp;
use std::sync::Arc;

pub async fn manage(
    node: &Arc<Node>,
    resident: ResidentId,
    machine: Option<Written>,
    action: Management,
) -> Result<(Address, Managed), Failure> {
    let local = machine.is_none() && node.residents.about(&resident).is_some();
    let machine = match machine {
        Some(machine) => machine,
        None if node.residents.about(&resident).is_some() => node.machine().into(),
        None => crate::colleagues::find_for(node, &Acting::Here, &resident).await?,
    };
    let managed = if local || node.is_here(&machine) {
        here(node, &resident, action).await?
    } else {
        if matches!(action, Management::Mcp { .. }) {
            let (link, account) =
                node.speaking(&Acting::Here, &machine, Purpose::Agents).map_err(|e| Failure::new(e.to_string()))?;
            crate::mcp::supports(node, &link, &account, &machine.name).await?;
        }
        let call = RemoteCall::Manage { resident: resident.clone(), action };
        match node
            .call(&Acting::Here, &machine, Purpose::Agents, call)
            .await
            .map_err(|error| Failure::new(error.to_string()))??
        {
            RemoteReply::Managed(managed) => managed,
            _ => return Err(Failure::new(format!("{machine} answered something other than a management result"))),
        }
    };
    Ok((Address { machine: machine.name, resident }, managed))
}

pub async fn here(node: &Arc<Node>, id: &ResidentId, action: Management) -> Result<Managed, Failure> {
    let _doing = fastctx_platform::doing::doing(format!("{action:?} of {id}"));
    let gate = node.residents.management(id)?;
    if matches!(action, Management::Sleep) {
        node.residents.active(id)?;
        if busy(node, id) {
            return Err(Failure::new("this resident is busy or starting a turn and cannot sleep"));
        }
    }
    // A turn it runs is stopped before it is archived, outside the gate:
    // what ends a turn may wait on it, as a tool call it answers does.
    if matches!(action, Management::Archive) && node.residents.removed(id).is_none() {
        crate::talk::stop_and_wait(node, id).await?;
    }
    let _exclusive = crate::gate_waits::alone(node, id, &gate).await?;
    if !matches!(action, Management::Archive | Management::Restore) {
        node.residents.active(id)?;
    }
    if matches!(action, Management::Archive)
        && node.residents.removed(id).is_none()
        && node.residents.now(id).is_some_and(|now| now.turn.is_some())
    {
        return Err(Failure::new("it began a turn again as it was being archived; nothing was changed"));
    }
    let (about, town) = node.residents.about(id).ok_or_else(|| Failure::new("unknown resident"))?;
    if matches!(action, Management::Wake) {
        crate::homes::available(node, &about)?;
    }
    // A background birth can already be visible before its CLI returns the
    // session ID. Removing it then would let the later registration create
    // a different resident for the same session.
    if about.world_started && about.session.is_none() {
        return Err(Failure::new(
            "the host has not identified this resident's session yet; management cannot act until that identity is known; its World records were left unchanged",
        ));
    }
    let mut result = Managed {
        removed: node.residents.removed(id),
        town,
        activity: node.residents.now(id).map(|now| now.activity),
        steps: Vec::new(),
        notification: None,
    };
    match action {
        Management::Archive => {
            let host = match (about.host.agent.as_str(), &about.session, about.world_started) {
                ("codex", Some(thread), _) => {
                    crate::codex::claim(node, thread, id).await;
                    match node.codex.of(node, &about) {
                        Ok(codex) => codex.archive(thread).await.map_err(Failure::new),
                        Err(failure) => Err(failure),
                    }
                }
                ("claude", Some(session), _) => {
                    match crate::claude::management::background_session(node, about.home.as_deref(), session).await {
                        Ok(Some(_)) => {
                            crate::claude::management::stop(node, about.home.as_deref(), id, session, false).await
                        }
                        Ok(None) => {
                            step(&mut result, "host", Outcome::Unsupported, "Claude Code exposes no background management ID for this session; its window remains open.".into());
                            Ok(())
                        }
                        Err(error) => Err(error),
                    }
                }
                _ => {
                    step(
                        &mut result,
                        "host",
                        Outcome::Unsupported,
                        "The World cannot close this user-opened host session; its window remains open.".into(),
                    );
                    Ok(())
                }
            };
            if result.steps.is_empty() {
                completed(
                    &mut result,
                    "host",
                    host,
                    "The host archived the thread or stopped its background session; World jobs keep running.",
                );
            }
            match node.residents.remove(&node.world, id) {
                Ok(path) => step(
                    &mut result,
                    "records",
                    Outcome::Done,
                    format!(
                        "Records kept in {path} until the conversation is deleted. Its jobs keep running. This session's subsequent World tool calls are refused."
                    ),
                ),
                Err(error) => step(&mut result, "records", Outcome::Failed, format!("Recording the removal: {error}")),
            }
            result.removed = node.residents.removed(id);
            if result.removed.is_some() {
                crate::desk::removed(node, id);
                crate::composer::removed(node, id);
                if about.host.agent.as_str() == "claude"
                    && let Some(session) = &about.session
                {
                    crate::claude::control::ended(node, session);
                    crate::claude::removed(node, id);
                }
                crate::colleagues::left(node, id.clone());
            }
        }
        Management::Restore => {
            if result.removed.is_none() {
                step(&mut result, "records", Outcome::Done, "It is not archived.".into());
                return Ok(result);
            }
            let host = match (about.host.agent.as_str(), &about.session) {
                ("codex", Some(thread)) => match node.codex.of(node, &about) {
                    Ok(codex) => codex.unarchive(thread).await.map_err(Failure::new),
                    Err(failure) => Err(failure),
                },
                // Claude Code's background session, stopped when it was
                // archived, is started again when it is next spoken to.
                _ => Ok(()),
            };
            completed(&mut result, "host", host, "The host's record of it is out of its archive.");
            match node.residents.restore(&node.world, id) {
                Ok(()) => step(
                    &mut result,
                    "records",
                    Outcome::Done,
                    "It is listed again where it was, and can be spoken to.".into(),
                ),
                Err(error) => {
                    step(&mut result, "records", Outcome::Failed, format!("Recording that it is back: {error}"))
                }
            }
            result.removed = node.residents.removed(id);
        }
        Management::Sleep | Management::Wake => {
            let sleeping = matches!(action, Management::Sleep);
            if sleeping && busy(node, id) {
                return Err(Failure::new("this resident is busy or starting a turn and cannot sleep"));
            }
            let manageable = if about.host.agent.as_str() == "claude" {
                match &about.session {
                    Some(session) => {
                        match crate::claude::management::background_session(node, about.home.as_deref(), session).await
                        {
                            Ok(session) => session.is_some(),
                            Err(error) => {
                                step(&mut result, "host", Outcome::Failed, error.message);
                                return Ok(result);
                            }
                        }
                    }
                    None => false,
                }
            } else {
                about.world_started
            };
            if !manageable {
                step(
                    &mut result,
                    "host",
                    Outcome::Unsupported,
                    "The World cannot unload or restart this user-opened session; use the host where it is open."
                        .into(),
                );
            } else if let Some(session) = &about.session {
                let effect: Result<bool, Failure> = match (about.host.agent.as_str(), sleeping) {
                    ("codex", true) => match node.codex.of(node, &about) {
                        Ok(codex) => codex.sleep(session).await.map_err(Failure::new),
                        Err(failure) => Err(failure),
                    },
                    ("codex", false) => match node.codex.of(node, &about)?.prepare(session).await {
                        Ok(missing) => {
                            if let Some(why) = missing {
                                step(&mut result, "tools", Outcome::Failed, why);
                            }
                            Ok(true)
                        }
                        Err(error) => Err(Failure::new(error)),
                    },
                    ("claude", true) => crate::claude::management::stop(node, about.home.as_deref(), id, session, true)
                        .await
                        .map(|()| true),
                    ("claude", false) => {
                        crate::claude::management::wake(node, about.home.as_deref(), session).await.map(|()| true)
                    }
                    _ => {
                        step(
                            &mut result,
                            "host",
                            Outcome::Unsupported,
                            "This host has no supported lifecycle entrance.".into(),
                        );
                        return Ok(result);
                    }
                };
                match effect {
                    Ok(true) => {
                        if about.host.agent.as_str() == "claude" {
                            node.residents.set_claude_here(id, !sleeping);
                        }
                        step(&mut result, "host", Outcome::Done, if sleeping { "The host session is unloaded or stopped." } else { "The original session is present; no message was submitted." }.into());
                        let recorded = node.residents.change_now(&node.world, id, |now| {
                            if now.turn.is_none() {
                                now.activity = if sleeping { Activity::Asleep } else { Activity::Idle };
                                now.since = Timestamp::now();
                            }
                        }).map_err(|error| Failure::new(error.to_string()));
                        completed(&mut result, "records", recorded, "Presence recorded.");
                    }
                    Ok(false) => step(&mut result, "host", Outcome::Pending, "Codex unsubscribed this connection. The thread is still loaded: its idle delay or another client's subscription prevents unloading. Presence changes only when Codex reports thread/closed; speaking or waking subscribes again.".into()),
                    Err(error) => step(&mut result, "host", Outcome::Failed, error.message),
                }
            } else {
                step(&mut result, "host", Outcome::Failed, "The host has not identified this session yet.".into());
            }
        }
        Management::Move { town } => {
            let brief = if let Some(town) = &town {
                crate::town::project::spans_here(node, town).await?;
                crate::town::map::brief(node, town).await?
            } else {
                "You are in no project now. `fastctx project list` lists the projects.".into()
            };
            let changed = match node.residents.relocate(&node.world, id, town.as_ref()) {
                Ok(changed) => changed,
                Err(error) => {
                    let current = node.residents.town_of(id).flatten();
                    result.town = current.clone();
                    step(
                        &mut result,
                        "records",
                        Outcome::Failed,
                        format!(
                            "Saving project membership failed: {error}; current project: {}",
                            current.as_ref().map_or("outside", TownName::as_str)
                        ),
                    );
                    return Ok(result);
                }
            };
            result.town = town.clone();
            step(
                &mut result,
                "records",
                Outcome::Done,
                if changed { "Project membership changed." } else { "This is already its project." }.into(),
            );
            if about.world_started
                && about.host.agent.as_str() == "codex"
                && let Some(thread) = &about.session
            {
                match crate::colleagues::thread_spec(
                    node,
                    id,
                    &about.origin,
                    town.as_ref(),
                    &about.acting,
                    about.home.as_deref(),
                )
                .await
                {
                    Ok((spec, _)) => match node.codex.of(node, &about) {
                        Ok(codex) => codex.adopt(thread, spec),
                        Err(failure) => step(&mut result, "host", Outcome::Failed, failure.message),
                    },
                    Err(error) => step(
                        &mut result,
                        "host",
                        Outcome::Failed,
                        format!(
                            "Project membership changed, but preparing the next Codex resume instructions failed: {error}"
                        ),
                    ),
                }
            }
            // Delivered as the World's words, under a line saying so: it is
            // not repeated here.
            let moved = match &town {
                Some(town) => format!("The user moved you to the project {town}"),
                None => "The user took you out of your project".to_string(),
            };
            let text = format!("{moved}; this replaces what you were told before of where you work.\n\n{brief}");
            let delivery = Delivery {
                id: speech::new_id("project"),
                at: Timestamp::now(),
                notice: Notice::World { fact: WorldFact::Moved { to: town.clone() }, text },
            };
            match speech::tell_here_locked(node, id, delivery, speech::Owed::Nobody).await {
                Ok(landed) => {
                    result.notification = Some(landed);
                }
                Err(error) => step(&mut result, "notification", Outcome::Failed, error.message),
            }
        }
        Management::Mcp { off } => {
            for name in &off {
                fastctx_types::profile::name_of(name).map_err(Failure::not_run)?;
            }
            let mut options = crate::session_options::read(node, id)?;
            options.mcp_off = off.clone();
            // About is authoritative once the conversation exists. A restart
            // reads it even if writing the secondary options file was interrupted.
            node.residents.set_mcp_off(&node.world, id, off).map_err(|e| Failure::new(e.to_string()))?;
            let options_path = node.world.conversation(id).session_options();
            fastctx_platform::ensure_private_dir(options_path.parent().expect("options directory"))
                .map_err(|e| Failure::new(e.to_string()))?;
            fastctx_platform::publish(
                &options_path,
                &serde_json::to_vec(&options).unwrap(),
                fastctx_platform::Permissions::Private,
            )
            .map_err(|e| Failure::new(e.to_string()))?;
            if about.host.agent.as_str() == "claude" {
                crate::profile::session::claude(node, id, false)?;
            } else {
                crate::codex::moved(node, id).await;
            }
            step(&mut result, "records", Outcome::Done, "MCP selection saved for the next session load.".into());
        }
        Management::Focus { focus } => {
            crate::focus::pointable(&about)?;
            let changed = match node.residents.set_focus(&node.world, id, focus.clone()) {
                Ok(changed) => changed,
                Err(error) => {
                    step(
                        &mut result,
                        "records",
                        Outcome::Failed,
                        format!("Saving its machines and folders failed: {error}"),
                    );
                    return Ok(result);
                }
            };
            step(
                &mut result,
                "records",
                Outcome::Done,
                if changed { "Machines and folders changed." } else { "These are already its machines and folders." }
                    .into(),
            );
            if changed {
                match crate::focus::changed(node, id, &about, &focus).await {
                    Ok(delivery) => match speech::tell_here_locked(node, id, delivery, speech::Owed::Nobody).await {
                        Ok(landed) => result.notification = Some(landed),
                        Err(error) => step(&mut result, "notification", Outcome::Failed, error.message),
                    },
                    Err(error) => step(&mut result, "notification", Outcome::Failed, error.message),
                }
            }
        }
    }
    result.activity = node.residents.now(id).map(|now| now.activity);
    Ok(result)
}

fn step(result: &mut Managed, stage: &str, outcome: Outcome, detail: String) {
    result.steps.push(ManagementStep { stage: stage.into(), outcome, detail });
}

fn busy(node: &Node, id: &ResidentId) -> bool {
    node.residents.now(id).is_some_and(|now| now.activity == Activity::Busy || now.turn.is_some())
}

fn completed(result: &mut Managed, stage: &str, effect: Result<(), Failure>, detail: &str) {
    match effect {
        Ok(()) => step(result, stage, Outcome::Done, detail.into()),
        Err(error) => step(result, stage, Outcome::Failed, error.message),
    }
}
