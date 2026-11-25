//! A colleague in a Claude Code login (`world-colleagues.md`): asked as one
//! in a Codex login is (`crate::colleagues::ask_here`), it is a session the
//! World starts as it starts a conversation of the user's (`open_here`),
//! born of the asker's words instead of the user's. They are its first
//! words, and the end of the turn they start is the asker's answer; what it
//! is told of itself is a colleague's, by its preset, with what the account
//! adds to its rules. Its session is started apart from the call that asked
//! for it, which hears its id at once.

use std::sync::Arc;

use fastctx_types::access::Acting;
use fastctx_types::collab::Started;
use fastctx_types::ipc::Failure;
use fastctx_types::residents::{
    About, Activity, BornAs, Delivery, Host, MessageKind, MessageLine, Notice, Now, Origin, Pending, PendingState,
};
use fastctx_types::session::{Speaker, TurnEnd, TurnHow};
use fastctx_types::{Address, AgentName, ResidentId, TownName};
use jiff::Timestamp;

use crate::Node;

/// What a colleague in a Claude Code login is asked by: the asker, the turn
/// it asked in, whom it works for, where, under what label and what.
pub(crate) struct Asked {
    pub asker: Address,
    pub requester_turn: Option<String>,
    pub acting: Acting,
    pub town: Option<TownName>,
    pub label: String,
    pub prompt: String,
}

/// Starts the colleague: its id, and how many wait ahead of its first turn
/// where it waits for its callee, its asker or its login to have room.
pub(crate) async fn born(
    node: &Arc<Node>,
    asked: Asked,
    started: Started,
    admitted: Option<Vec<BornAs>>,
) -> Result<(ResidentId, Option<u32>), Failure> {
    let Asked { asker, requester_turn, acting, town, label, prompt } = asked;
    let changes_it = |failure: Failure| {
        Failure::not_run(format!("{}; the user changes it on the Model collaboration page", failure.message))
    };
    let home = crate::homes::listed(node, "claude", started.callee.login.home.as_deref()).map_err(changes_it)?;
    // Asked before anything of the colleague is kept or its login started.
    crate::logins::workbench::may_start(node, &acting, "claude", home.as_deref())?;
    let launching = super::command(node, home.as_deref())?;
    super::oldest::check(node, &launching).await?;
    let offering = crate::logins::models::Offering::of(node, home.as_deref())?;
    let agent = AgentName::try_from("claude").expect("a valid agent name");
    let born_as = {
        let (giving, home, working, agent) = (node.clone(), home.clone(), acting.clone(), agent.clone());
        fastctx_platform::task::blocking(move || crate::assists::born_as(&giving, &working, &agent, home.as_deref()))
            .await?
    };
    // Let in by one right, it is not born by another (codex-87c8bbd9).
    if admitted.as_ref().is_some_and(|admitted| *admitted != born_as) {
        return Err(Failure::refused(
            "the right the colleague was asked by no longer stands: the assistance or the account it was asked under has changed since",
        ));
    }
    // It runs on the model its callee names, which its login must offer, else
    // on the one the account starts its conversations on.
    let model = match started.callee.model.clone() {
        Some(model) => {
            offering.require(&model).map_err(changes_it)?;
            model
        }
        None => {
            crate::profile::session::ready(node, &acting).await?;
            let profile = crate::profile::session::get(node, &acting);
            offering.starting(profile.claude.get("model").and_then(serde_json::Value::as_str))?
        }
    };
    let options = fastctx_types::agents::SessionOptions {
        mcp_off: Default::default(),
        reasoning_effort: crate::logins::models::effort_for(&model, started.callee.effort.clone()),
        model: Some(model),
    };
    // Held until its record is written (`purges::permit`).
    let _permit = crate::purges::permit(node, &born_as)?;
    let id = node.residents.new_id(&agent)?;
    crate::record_migration::born(node, &id);
    crate::collab::keep(node, &id, &started).await?;
    crate::session_options::keep(node, &id, &options).await?;
    // Where the requester works is only a first guess at where the colleague
    // speaks from; without it the colleague still chooses its own place.
    if let Err(error) = crate::placement::location::inherit(node, &id, &acting, &asker).await {
        node.log.line(format_args!("starting {id} where its requester works: {}", error.message));
    }
    let origin = Origin::Colleague { asked_by: asker.clone(), label };
    let _birth = crate::here::born(node, &id, acting.clone(), origin.clone());
    crate::profile::session::ready(node, &acting).await?;
    // A login assisting another account starts without the browser of this
    // machine, as kept now.
    crate::assists::host::starting(node, &id, &acting)?;
    let (mut words, heard) =
        crate::colleagues::instructions(node, &id, town.as_ref(), &origin, &acting, false, started.callee.kind())
            .await?;
    words.push_str(&crate::colleagues::added_rules(&started));
    let born_at = Timestamp::now();
    let about = About {
        side: false,
        forked_from: None,
        mcp_off: Default::default(),
        idle_compaction: Default::default(),
        id: id.clone(),
        host: Host { agent, client: "claude-code".to_string(), version: String::new() },
        machine: node.machine(),
        born_at,
        origin,
        session: None,
        world_started: true,
        native_resumed: false,
        model: None,
        acting,
        focus: Default::default(),
        home,
        // The login it is born in is read as its session is started.
        login: None,
        born_as: Some(born_as),
        callee: Some(started.callee.name.clone()),
        born_shared: crate::timeline::born_shared::now(node),
    };
    // Its first turn waits where its callee, its asker or its login has no
    // room for it, its words kept (`queued`).
    let admitted = crate::turn_slots::admit(node, &id, &about);
    let now = Now {
        activity: if admitted.is_ok() { Activity::Busy } else { Activity::Idle },
        since: born_at,
        turn: None,
        last_turn: None,
        owed: Default::default(),
        answering: Vec::new(),
        trouble: None,
        desk: false,
        desk_since: None,
        waiting_for_user: false,
        waiting_since: None,
        title: None,
        last_tool_at: None,
        first_words: None,
        last_said: None,
        last_compacted: false,
        last_heard_user: None,
        asked: None,
        told: None,
    };
    if let Some(town) = &town {
        crate::town::access::check(node, &about.acting, town)?;
    }
    node.residents
        .create(&node.world, about, town, now)
        .map_err(|error| Failure::new(format!("recording the colleague: {error}")))?;
    crate::context::instructed(node, &id, &heard).await.map_err(|error| Failure::new(error.to_string()))?;
    node.residents.set_instructed(&id, heard);
    let directory = node.world.conversation(&id);
    let written = fastctx_platform::disk::write(directory.claude_settings(), super::settings_json(&id))
        .and_then(|()| fastctx_platform::disk::write(directory.claude_instructions(), words));
    written.map_err(|error| Failure::new(format!("writing the colleague's settings: {error}")))?;
    let asked = MessageLine {
        at: born_at,
        what: MessageKind::Asked { from: asker.clone(), text: prompt.clone(), requester_turn },
    };
    // The asker's words are its first, kept before the asker hears of it, and
    // the end of the turn they start is the asker's answer.
    let first = Pending {
        delivery: Delivery {
            id: crate::speech::new_id("words"),
            at: born_at,
            notice: Notice::Words {
                from: Speaker::Resident { address: asker.clone() },
                text: prompt,
                goal_report: None,
                goal_question: None,
                told: None,
            },
        },
        readers: vec![asker],
        state: PendingState::Waking,
        working: false,
        session: None,
        since: Some(born_at),
    };
    let (keeping, kept_first, logging) = (directory.clone(), first.clone(), node.clone());
    fastctx_platform::task::spawn_blocking(move || {
        if let Err(error) = crate::residents::append_message(&keeping, &asked) {
            logging.log.line(format_args!("recording the colleague's first request: {error}"));
        }
        crate::inbox::keep(&keeping, &kept_first)
    })
    .await
    .map_err(|error| Failure::new(error.to_string()))?
    .map_err(|error| Failure::new(fastctx_types::collab_prompt::first_request_failed(&id, error)))?;
    if let Err(queued) = admitted {
        super::queued::mark(&directory)
            .map_err(|error| Failure::new(format!("keeping the colleague's first turn in line: {error}")))?;
        super::queued::wait(node, id.clone());
        return Ok((id, Some(queued.ahead)));
    }
    let (starting, colleague) = (node.clone(), id.clone());
    let answering = node.handoff.call();
    fastctx_platform::task::spawn(async move {
        let Some(_answering) = answering else { return };
        launch(&starting, &colleague, first).await;
    });
    Ok((id, None))
}

/// Starts the colleague's session with its first words. One that cannot be
/// started yet waits in line as a new conversation's first turn does; one
/// that failed is its first turn failed, which its asker hears.
async fn launch(node: &Arc<Node>, id: &ResidentId, first: Pending) {
    let failure = match super::prepare_launch(node, id).await {
        Err(failure) => match super::back_in_line(node, id, failure) {
            Ok(_) => return,
            Err(failure) => failure,
        },
        Ok(prepared) => match super::begin_launch(node, id, prepared).await {
            Ok(_) => return,
            Err(failure) => failure,
        },
    };
    node.log.line(format_args!("starting the colleague {id}: {}", failure.message));
    let directory = node.world.conversation(id);
    let why = failure.message;
    crate::turns::unsettled(node, id, &directory, first, TurnHow::Failed, why.clone()).await;
    crate::turns::ended(
        node,
        id,
        "",
        TurnEnd {
            facts: None,
            error: None,
            how: TurnHow::Failed,
            reason: Some(why),
            duration_ms: None,
            last_message: None,
        },
    )
    .await;
}
