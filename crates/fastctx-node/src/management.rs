//! User management follows the resident's machine. Host effects and World
//! membership are reported separately so partial completion stays visible.

use crate::{Node, speech};
use fastctx_types::access::{Acting, Purpose};
use fastctx_types::ipc::Failure;
use fastctx_types::management::{Managed, Management, ManagementOutcome as Outcome, ManagementStep};
use fastctx_types::remote::{RemoteCall, RemoteReply};
use fastctx_types::residents::{Activity, Delivery, Notice};
use fastctx_types::session::Speaker;
use fastctx_types::{Address, MachineName, ResidentId};
use jiff::Timestamp;
use std::sync::Arc;

pub async fn manage(
    node: &Arc<Node>,
    resident: ResidentId,
    machine: Option<MachineName>,
    action: Management,
) -> Result<(Address, Managed), Failure> {
    let machine = match machine {
        Some(machine) => machine,
        None if node.residents.about(&resident).is_some() => node.machine(),
        None => crate::colleagues::find_for(node, &Acting::Here, &resident).await?,
    };
    let managed = if machine == node.machine() {
        here(node, &resident, action).await?
    } else {
        let call = RemoteCall::Manage { resident: resident.clone(), action };
        match node
            .call(&Acting::Here, &(&machine).into(), Purpose::Agents, call)
            .await
            .map_err(|error| Failure::new(error.to_string()))??
        {
            RemoteReply::Managed(managed) => managed,
            _ => return Err(Failure::new(format!("{machine} answered something other than a management result"))),
        }
    };
    Ok((Address { machine, resident }, managed))
}

pub async fn here(node: &Arc<Node>, id: &ResidentId, action: Management) -> Result<Managed, Failure> {
    let gate = node.residents.management(id)?;
    if matches!(action, Management::Sleep) {
        node.residents.active(id)?;
        if busy(node, id) {
            return Err(Failure::new("this resident is busy or starting a turn and cannot sleep"));
        }
    }
    let _exclusive = gate.write().await;
    if !matches!(action, Management::Remove) {
        node.residents.active(id)?;
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
        Management::Remove => {
            let host = match (about.host.agent.as_str(), &about.session, about.world_started) {
                ("codex", Some(thread), _) => {
                    crate::codex::claim(node, thread, id).await;
                    match node.codex.of(&about) {
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
                Ok(paths) => step(
                    &mut result,
                    "records",
                    Outcome::Done,
                    format!(
                        "Records archived in {}. Jobs and output retain their original paths so running jobs continue. This session's subsequent World tool calls are refused.",
                        paths.join(", ")
                    ),
                ),
                Err(error) => step(&mut result, "records", Outcome::Failed, format!("Archiving records: {error}")),
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
                    ("codex", true) => match node.codex.of(&about) {
                        Ok(codex) => codex.sleep(session).await.map_err(Failure::new),
                        Err(failure) => Err(failure),
                    },
                    ("codex", false) => match node.codex.of(&about)?.prepare(session).await {
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
                    step(
                        &mut result,
                        "records",
                        Outcome::Failed,
                        format!(
                            "The project move did not finish: {error}. Completed filesystem steps remain recorded for recovery on restart."
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
                    about.works_on.as_ref(),
                )
                .await
                {
                    Ok((spec, _)) => match node.codex.of(&about) {
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
                notice: Notice::Words { from: Speaker::World, text },
            };
            match speech::tell_here_locked(node, id, delivery, speech::Owed::Nobody).await {
                Ok(landed) => {
                    result.notification = Some(landed);
                }
                Err(error) => step(&mut result, "notification", Outcome::Failed, error.message),
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
