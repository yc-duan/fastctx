//! The user speaking to a resident anywhere in the World and watching it
//! (`world-colleagues.md`, 用户在任何地方和居民说话).
//!
//! The user's words go in as the user's: the machine they were typed on
//! signs the call that carries them, and the session reads them as said by
//! the user on that machine. Nobody is owed the turn they go into; the user
//! watches it. Watching hands over what happened before, as far as the
//! session's host keeps it, then each event as it happens. A resident on
//! another machine is watched over a data connection from its machine, each
//! piece signed there, bound to this watch and numbered, so the hub in
//! between can neither make one up, drop one, nor play an old one again.

use std::sync::Arc;

use fastctx_net::data::Stream;
use fastctx_net::link::{Feed, Link};
use fastctx_net::sign::Domain;
use fastctx_types::access::{Acting, Purpose, Written};
use fastctx_types::agents::Choices;
use fastctx_types::attachments::Attachment;
use fastctx_types::ipc::Failure;
use fastctx_types::layout::ResidentDir;
use fastctx_types::remote::{RemoteCall, RemoteReply};
use fastctx_types::residents::{About, Activity, Host, Landed, Now, Origin};
use fastctx_types::session::{SessionEvent, Watched};
use fastctx_types::{Address, AgentName, MachineName, ResidentId, TownName};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::Node;
use crate::speech::{self, Owed};

/// One piece of a watch from another machine, as that machine signs it.
#[derive(Serialize, Deserialize)]
struct Piece {
    watch: u64,
    n: u64,
    watched: Watched,
}

/// The user's words, typed on this machine, to a resident on any machine.
pub async fn talk(
    node: &Arc<Node>,
    to: ResidentId,
    machine: Option<MachineName>,
    text: String,
    attachments: Vec<Attachment>,
    files_on: Option<Written>,
) -> Result<(Address, Landed), Failure> {
    let here = node.machine();
    let machine = match machine {
        Some(machine) => machine,
        None => locate(node, &to).await?,
    };
    if machine == here {
        let landed = talk_here(node, &here, &to, text, attachments, files_on.as_ref()).await?;
        return Ok((Address { machine: here, resident: to }, landed));
    }
    let call = RemoteCall::Talk { to: to.clone(), text, attachments, files_on };
    match node
        .call(&Acting::Here, &(&machine).into(), Purpose::Agents, call)
        .await
        .map_err(|failure| Failure::new(failure.to_string()))?
    {
        Ok(RemoteReply::Said { landed }) => Ok((Address { machine, resident: to }, landed)),
        Ok(_) => Err(Failure::new(format!("{machine} answered something other than a talk"))),
        Err(failure) => Err(failure),
    }
}

/// The user's words, typed on `typed_on`, to a resident of this machine;
/// its attached files other than images go to `files_on` when the user
/// chose.
pub async fn talk_here(
    node: &Arc<Node>,
    typed_on: &MachineName,
    to: &ResidentId,
    text: String,
    attachments: Vec<Attachment>,
    files_on: Option<&Written>,
) -> Result<Landed, Failure> {
    crate::attachments::resolve(node, to, &attachments).await?;
    let attachments = crate::attachments::place(node, to, attachments, files_on).await?;
    let delivery = crate::attachments::delivery(typed_on, text, attachments);
    speech::tell_here(node, to, delivery, Owed::Nobody).await
}

/// Watches a resident on any machine. What it hands over comes on the
/// returned channel until the watch ends, with why when something went
/// wrong; dropping the channel ends the watch.
pub async fn watch(
    node: &Arc<Node>,
    resident: ResidentId,
    machine: Option<MachineName>,
    composer: Option<String>,
) -> Result<mpsc::UnboundedReceiver<Result<Watched, String>>, Failure> {
    let here = node.machine();
    let machine = match machine {
        Some(machine) => machine,
        None => locate(node, &resident).await?,
    };
    let (sender, receiver) = mpsc::unbounded_channel();
    if machine == here {
        let lease = match composer {
            Some(name) => Some(crate::composer::watch(node, &resident, name).await?),
            None => None,
        };
        let (history, mut events) = if lease.is_some() {
            (Vec::new(), node.watchers.watch_composer(&resident))
        } else {
            watch_here(node, &resident).await?
        };
        if lease.is_some() {
            crate::composer::snapshot(node, &resident).await?;
        }
        let _ = sender.send(Ok(Watched::History { events: history }));
        tokio::spawn(async move {
            let _lease = lease;
            loop {
                tokio::select! {
                    event = events.recv() => match event {
                        Some(event) => if sender.send(Ok(Watched::Live { event })).is_err() { return },
                        None => return,
                    },
                    () = sender.closed() => return,
                }
            }
        });
        return Ok(receiver);
    }
    let watch = u64::from_le_bytes(
        fastctx_net::sign::random::<8>().map_err(|error| Failure::new(format!("naming the watch: {error}")))?,
    );
    let written: Written = (&machine).into();
    let links = node.locate(&written)?;
    let feed = {
        let (sender, from) = (sender.clone(), machine.clone());
        move || -> Feed {
            let (feeding, links, from) = (sender.clone(), links.clone(), from.clone());
            Box::new(move |mut stream: Stream| {
                Box::pin(async move {
                    let mut n = 0;
                    loop {
                        let signed = tokio::select! {
                            signed = stream.hear::<fastctx_net::sign::Signed>() => signed?,
                            () = feeding.closed() => return Ok(()),
                        };
                        let piece: Piece = unseal(&links, &from, &signed).await?;
                        if piece.watch != watch || piece.n != n {
                            return Err(format!(
                                "{from} sent a piece out of place (watch {:x}, number {}; expected {watch:x}, {n})",
                                piece.watch, piece.n
                            ));
                        }
                        n += 1;
                        if feeding.send(Ok(piece.watched)).is_err() {
                            return Ok(());
                        }
                    }
                })
            })
        }
    };
    let call = RemoteCall::Watch { resident, watch, composer };
    let node = node.clone();
    tokio::spawn(async move {
        let result = tokio::select! {
            result = node.call_with_stream(&Acting::Here, &written, Purpose::Agents, call, feed) => result,
            () = sender.closed() => return,
        };
        let ended = match result {
            Ok(Ok(_)) => return,
            Ok(Err(failure)) => failure.message,
            Err(failure) => failure.to_string(),
        };
        let _ = sender.send(Err(ended));
    });
    Ok(receiver)
}

/// A resident of this machine: its history, and the events from now on.
/// The watch starts before the history is read, so nothing falls between;
/// what is in both is the watcher's to recognise.
pub async fn watch_here(
    node: &Arc<Node>,
    resident: &ResidentId,
) -> Result<(Vec<SessionEvent>, mpsc::UnboundedReceiver<SessionEvent>), Failure> {
    let (about, town) = node
        .residents
        .about(resident)
        .ok_or_else(|| Failure::new(format!("no resident {resident} lives on {}", node.machine())))?;
    let mut events = node.watchers.watch(resident);
    let directory = node.world.resident(town.as_ref(), resident);
    let recorded =
        |directory: &ResidentDir| crate::delivery::history(directory).map_err(|error| Failure::new(error.to_string()));
    let (recorded, hosted) = match &about.session {
        Some(thread) if about.world_started && about.host.agent.as_str() == "codex" => {
            crate::homes::available(node, &about)?;
            let codex = node.codex.of(&about)?;
            let history = codex.history(thread).await.map_err(Failure::new)?;
            codex.flush_events(thread).await.map_err(Failure::new)?;
            // Reverted turns remain in the World's audit record, but no
            // longer belong to the host's current conversation.
            let recorded = crate::codex::watch::recorded(recorded(&directory)?, &history);
            // A request can arrive and resolve while history is loading.
            // Only currently unanswered requests may enter a new view; the
            // subscription already covers changes after this snapshot.
            events = crate::codex::watch::requests(codex, thread.clone(), events);
            (recorded, Some(history))
        }
        // Claude Code's own record of the session, placed in the turns the
        // World saw, which the World's record says.
        _ if about.host.agent.as_str() == "claude" => {
            let mut recorded = recorded(&directory)?;
            let removed: std::collections::HashSet<_> = recorded
                .iter()
                .flat_map(|event| match &event.happened {
                    fastctx_types::session::Happened::Changed {
                        change: fastctx_types::session::SessionChange::HistoryCut { removed_turns },
                        ..
                    } => removed_turns.clone(),
                    _ => Vec::new(),
                })
                .collect();
            recorded.retain(|event| turn_of(&event.happened).is_none_or(|turn| !removed.contains(turn)));
            let hosted = crate::claude::history::hosted(node, resident, &recorded).await;
            (recorded, Some(hosted))
        }
        _ => (recorded(&directory)?, None),
    };
    let mut history = history(recorded, hosted, about.host.agent.as_str());
    if about.world_started
        && about.host.agent.as_str() == "codex"
        && let Some(thread) = &about.session
    {
        history.extend(node.codex.of(&about)?.requests(thread));
    }
    Ok((history, events))
}

/// A session's history: what the World kept of it, and what its host keeps
/// that the World does not (`crate::turns::recorded`). Where both tell a
/// turn's start or end, the World's account stands: it knows why a turn was
/// cut short. Words the World sent are told by its own record, which knows
/// who said them; the host's copy of them carries the World's head note.
fn history(recorded: Vec<SessionEvent>, hosted: Option<Vec<SessionEvent>>, agent: &str) -> Vec<SessionEvent> {
    use fastctx_types::session::Happened;
    let Some(hosted) = hosted else {
        return recorded_history(recorded);
    };
    let sent_turns: std::collections::HashSet<_> = recorded
        .iter()
        .filter_map(|event| match &event.happened {
            Happened::Heard { turn: Some(turn), .. } => Some(turn.as_str()),
            _ => None,
        })
        .collect();
    let delivered: std::collections::HashSet<_> = recorded
        .iter()
        .filter_map(|event| match &event.happened {
            Happened::Heard { delivery: Some(id), .. } => Some(id.clone()),
            _ => None,
        })
        .collect();
    let mut history: Vec<SessionEvent> = hosted
        .into_iter()
        .flat_map(fastctx_types::session::attachments::restore)
        .filter(|event| match &event.happened {
            // Words with attachments are known by their delivery.
            Happened::Heard { delivery: Some(id), message: Some(message), .. } if !message.attachments.is_empty() => {
                !delivered.contains(id)
            }
            // Other words the World told are its own record's, except, for
            // Codex, in a turn the World has no record of sending into.
            Happened::Heard { turn, text, .. } => {
                !(fastctx_hands::told::is_told(text)
                    && (agent != "codex" || turn.as_deref().is_some_and(|turn| sent_turns.contains(turn))))
            }
            _ => true,
        })
        .collect();
    for event in recorded {
        let same = history.iter().position(|other| match (&event.happened, &other.happened) {
            (Happened::TurnStarted { turn: a }, Happened::TurnStarted { turn: b })
            | (Happened::TurnEnded { turn: a, .. }, Happened::TurnEnded { turn: b, .. }) => a == b,
            (
                Happened::Heard { turn: a, message: Some(a_message), .. },
                Happened::Heard { turn: b, message: Some(b_message), .. },
            ) => a == b && a_message.id == b_message.id,
            (Happened::Realtime { item: a, complete: ac }, Happened::Realtime { item: b, complete: bc }) => {
                ac == bc && a["id"] == b["id"] && a["realtimeSessionId"] == b["realtimeSessionId"]
            }
            _ => false,
        });
        if let Some(index) = same {
            history[index] = event;
            continue;
        }
        let turn = turn_of(&event.happened);
        let index = history
            .iter()
            .position(|other| {
                let same_turn = turn.is_some() && turn == turn_of(&other.happened);
                if same_turn && matches!(other.happened, Happened::TurnEnded { .. }) {
                    return true;
                }
                if same_turn && matches!(event.happened, Happened::TurnStarted { .. }) {
                    return true;
                }
                if matches!(other.happened, Happened::TurnStarted { .. }) && same_turn {
                    return false;
                }
                other.at > event.at
            })
            .unwrap_or(history.len());
        history.insert(index, event);
    }
    history.retain(|event| event.happened.kept());
    // Preserve the host's canonical timeline, including interleaved voice
    // items and steering input. Local facts were inserted at their source time.
    history
}

fn recorded_history(mut history: Vec<SessionEvent>) -> Vec<SessionEvent> {
    use fastctx_types::session::Happened;
    history.retain(|event| event.happened.kept());
    // The host's record keeps times in whole seconds, the World's to the
    // microsecond, so time alone would put an answer before the words that
    // asked for it. Each turn stands where it began, and within it come its
    // start, the words it heard, what was done in the order the record holds
    // it, and its end. What belongs to no turn stands at its own time.
    let mut began: std::collections::HashMap<String, jiff::Timestamp> = std::collections::HashMap::new();
    for event in &history {
        if let Some(turn) = turn_of(&event.happened) {
            began.entry(turn.to_string()).and_modify(|at| *at = (*at).min(event.at)).or_insert(event.at);
        }
    }
    history.sort_by_key(|event| {
        let at = match &event.happened {
            Happened::Finished { item, .. } => item.started_at.unwrap_or(event.at),
            _ => event.at,
        };
        let phase = match &event.happened {
            Happened::TurnStarted { .. } => 0,
            Happened::Heard { .. } => 1,
            Happened::TurnEnded { .. } => 3,
            _ => 2,
        };
        match turn_of(&event.happened).and_then(|turn| began.get(turn)) {
            Some(start) => (*start, phase, at),
            None => (at, 2, at),
        }
    });
    history
}

/// The turn an event belongs to, where it says.
pub(crate) fn turn_of(happened: &fastctx_types::session::Happened) -> Option<&str> {
    use fastctx_types::session::Happened;
    match happened {
        Happened::TurnStarted { turn }
        | Happened::TurnEnded { turn, .. }
        | Happened::Began { turn, .. }
        | Happened::Updated { turn, .. }
        | Happened::Grew { turn, .. }
        | Happened::Finished { turn, .. }
        | Happened::Steps { turn, .. } => (!turn.is_empty()).then_some(turn),
        Happened::Heard { turn, .. } | Happened::Health { turn, .. } | Happened::Changed { turn, .. } => {
            turn.as_deref()
        }
        Happened::Context { .. }
        | Happened::Titled { .. }
        | Happened::DeliveryUncertain { .. }
        | Happened::Realtime { .. }
        | Happened::RealtimeGrew { .. } => None,
    }
}

/// A piece of a watch, as the machine it came from signed it: its key is
/// the same on every hub it is on, so any of them can tell.
async fn unseal(links: &[Arc<Link>], from: &MachineName, signed: &fastctx_net::sign::Signed) -> Result<Piece, String> {
    let mut why = Vec::new();
    for link in links {
        match link.unseal(from, Domain::Event, signed).await {
            Ok(piece) => return Ok(piece),
            Err(error) => why.push(error),
        }
    }
    Err(why.join("; "))
}

/// Hands a watch of a resident of this machine to whoever asked through
/// `link`, over `stream`, until they close it; each piece is signed here.
pub async fn serve_watch(
    node: &Arc<Node>,
    link: &Arc<Link>,
    resident: ResidentId,
    watch: u64,
    composer: Option<String>,
    mut stream: Stream,
) -> Result<RemoteReply, Failure> {
    let opening = async {
        let lease = match composer {
            Some(name) => Some(crate::composer::watch(node, &resident, name).await?),
            None => None,
        };
        let (history, events) = if lease.is_some() {
            (Vec::new(), node.watchers.watch_composer(&resident))
        } else {
            watch_here(node, &resident).await?
        };
        if lease.is_some() {
            crate::composer::snapshot(node, &resident).await?;
        }
        Ok::<_, Failure>((lease, history, events))
    };
    let (_lease, history, mut events) = tokio::select! {
        result = opening => result?,
        () = stream.closed() => return Ok(RemoteReply::Watched),
    };
    let mut n = 0;
    let mut next = Some(Watched::History { events: history });
    loop {
        if let Some(watched) = next.take() {
            let signed = link.sign(Domain::Event, &Piece { watch, n, watched });
            if stream.say(&signed).await.is_err() {
                return Ok(RemoteReply::Watched);
            }
            n += 1;
        }
        tokio::select! {
            event = events.recv() => match event {
                Some(event) => next = Some(Watched::Live { event }),
                None => return Ok(RemoteReply::Watched),
            },
            () = stream.closed() => return Ok(RemoteReply::Watched),
        }
    }
}

/// Opens a session for the user on this machine or on `target`, working
/// for the account the user chose, or for this computer's logins where it
/// lives here and they chose none.
#[allow(clippy::too_many_arguments)]
pub async fn open(
    node: &Arc<Node>,
    agent: AgentName,
    town: Option<TownName>,
    target: Option<MachineName>,
    text: String,
    options: fastctx_types::agents::SessionOptions,
    attachments: Vec<Attachment>,
    choices: Choices,
) -> Result<(Address, Landed), Failure> {
    let here = node.machine();
    let acting = match &choices.account {
        Some(account) => {
            if !node.serves(&Acting::Here, &account.hub, &account.account) {
                return Err(Failure::not_run(format!(
                    "this computer is not logged in to the account {}",
                    account.account
                )));
            }
            Acting::one(account.hub.clone(), account.account.clone())
        }
        None => {
            // A conversation opened from the World's interface works for one
            // account; with one logged in here, that one.
            let logins: Vec<_> = node
                .links()
                .iter()
                .flat_map(|link| {
                    let hub = link.membership().id.clone();
                    let logins = link.view().me().map(|me| me.logins.clone()).unwrap_or_default();
                    logins
                        .into_iter()
                        .map(move |account| fastctx_types::access::HubAccount { hub: hub.clone(), account })
                })
                .collect();
            match logins.as_slice() {
                [] => Acting::Here,
                [only] => Acting::one(only.hub.clone(), only.account.clone()),
                _ => {
                    return Err(Failure::not_run(
                        "this computer is logged in to more than one account; choose the one the conversation works for",
                    ));
                }
            }
        }
    };
    match target.filter(|target| *target != here) {
        None => {
            let (id, landed) = open_here(node, &here, acting, agent, town, text, options, attachments, choices).await?;
            Ok((Address { machine: here, resident: id }, landed))
        }
        Some(target) => {
            let call = RemoteCall::Open {
                agent,
                town,
                text,
                model: options.model,
                reasoning_effort: options.reasoning_effort,
                attachments,
                works_on: choices.works_on,
                colleagues: choices.colleagues,
                home: choices.home,
                colleagues_home: choices.colleagues_home,
                files_on: choices.files_on,
            };
            let reply = node.call(&acting, &(&target).into(), Purpose::Agents, call).await;
            match reply.map_err(|failure| Failure::new(failure.to_string()))? {
                Ok(RemoteReply::Opened { resident, landed }) => Ok((Address { machine: target, resident }, landed)),
                Ok(_) => Err(Failure::new(format!("{target} answered something other than an opened session"))),
                // What went wrong there is said of that machine, not this one.
                Err(failure) => Err(Failure::new(format!("on {target}: {}", failure.message))),
            }
        }
    }
}

/// Opens a session on this machine for the user typing on `typed_on`, with
/// their words as its first, working for `acting`.
#[allow(clippy::too_many_arguments)]
pub async fn open_here(
    node: &Arc<Node>,
    typed_on: &MachineName,
    acting: Acting,
    agent: AgentName,
    town: Option<TownName>,
    text: String,
    options: fastctx_types::agents::SessionOptions,
    attachments: Vec<Attachment>,
    choices: Choices,
) -> Result<(ResidentId, Landed), Failure> {
    let works_on = choices.works_on.filter(|works_on| !node.is_here(works_on));
    // An agent lent to another account works on its machines, each command
    // naming one, and on no machine by default (`world-run.md`, 这里).
    let lent = node.works_here(&acting).is_err();
    match &works_on {
        Some(works_on) if lent => {
            return Err(Failure::not_run(format!(
                "this agent is lent to the account it works for, so each of its commands names its machine; it works on no machine by default, {works_on} included"
            )));
        }
        Some(works_on) => {
            node.speaking(&acting, works_on, Purpose::Hands)
                .map_err(|failure| Failure::not_run(format!("it cannot work on {works_on}: {failure}")))?;
        }
        None => {}
    }
    // The home it is born in must be one this machine has; the default one
    // is recorded as none, so that it follows the user's environment.
    let home = match choices.home {
        None => None,
        Some(dir) => {
            let env = crate::claude::environment()?;
            let user_home = fastctx_platform::user_home().map_err(|error| Failure::new(error.to_string()))?;
            let home = crate::homes::find(node, &env, &user_home, agent.as_str(), Some(&dir))?;
            (home.found != fastctx_types::agents::HomeFound::Default).then_some(home.dir)
        }
    };
    let opened = Opened {
        acting,
        works_on,
        colleagues: choices.colleagues,
        home,
        colleagues_home: choices.colleagues_home,
        files_on: choices.files_on,
    };
    match agent.as_str() {
        "codex" => {}
        "claude" => return crate::claude::open_here(node, typed_on, opened, town, text, options, attachments).await,
        _ => {
            return Err(Failure::new(format!(
                "the World opens codex and claude sessions; it cannot open {agent} ones"
            )));
        }
    }
    if let Some(town) = &town {
        crate::town::project::spans_here(node, town).await?;
    }
    let (id, _opening) = crate::attachments::opening(node, &agent, town.as_ref(), &attachments).await?;
    let _birth = crate::here::born(node, &id, opened.acting.clone(), opened.works_on.clone());
    let origin = Origin::User;
    crate::session_options::keep(node, &id, &options)?;
    if let Some(lives_on) = &opened.colleagues {
        crate::colleagues::setting::keep(node, &id, lives_on, opened.colleagues_home.clone())?;
    }
    let (spec, heard) = crate::colleagues::thread_spec(
        node,
        &id,
        &origin,
        town.as_ref(),
        &opened.acting,
        opened.works_on.as_ref(),
        opened.home.as_deref(),
    )
    .await?;
    let (thread, model) = node.codex.home(opened.home.as_deref())?.start_thread(spec).await.map_err(Failure::new)?;
    let born_at = Timestamp::now();
    let about = About {
        id: id.clone(),
        host: Host { agent, client: "codex app-server".to_string(), version: String::new() },
        machine: node.machine(),
        born_at,
        origin,
        session: Some(thread.clone()),
        world_started: true,
        model,
        acting: opened.acting,
        works_on: opened.works_on,
        home: opened.home,
    };
    let now = Now {
        activity: Activity::Idle,
        since: born_at,
        turn: None,
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
        asked: None,
    };
    node.residents
        .create(&node.world, about, town, now)
        .map_err(|error| Failure::new(format!("recording the session: {error}")))?;
    node.residents.set_heard(&id, heard);
    crate::codex::claim(node, &thread, &id).await;
    let landed = talk_here(node, typed_on, &id, text, attachments, opened.files_on.as_ref()).await?;
    Ok((id, landed))
}

/// Stops the turn a resident on any machine is running; returns where it
/// lives and whether a turn was running.
pub async fn stop(
    node: &Arc<Node>,
    resident: ResidentId,
    machine: Option<MachineName>,
) -> Result<(Address, bool), Failure> {
    let here = node.machine();
    let machine = match machine {
        Some(machine) => machine,
        None => locate(node, &resident).await?,
    };
    if machine == here {
        let stopped = stop_here(node, &resident).await?;
        return Ok((Address { machine: here, resident }, stopped));
    }
    let call = RemoteCall::Stop { resident: resident.clone() };
    let reply = node.call(&Acting::Here, &(&machine).into(), Purpose::Agents, call).await;
    match reply.map_err(|failure| Failure::new(failure.to_string()))? {
        Ok(RemoteReply::Stopped { stopped }) => Ok((Address { machine, resident }, stopped)),
        Ok(_) => Err(Failure::new(format!("{machine} answered something other than a stop"))),
        Err(failure) => Err(failure),
    }
}

/// Stops the turn a resident of this machine is running: a Codex thread of
/// this node's app-server, through the app-server; a Claude session, any
/// whose plugin is paired with the World, through its mod. A Codex session
/// of the user's own belongs to the program it is open in.
pub async fn stop_here(node: &Arc<Node>, resident: &ResidentId) -> Result<bool, Failure> {
    node.residents.active(resident)?;
    let (about, _) = node
        .residents
        .about(resident)
        .ok_or_else(|| Failure::new(format!("no resident {resident} lives on {}", node.machine())))?;
    match (about.host.agent.as_str(), about.world_started, &about.session) {
        ("codex", true, Some(thread)) => node.codex.of(&about)?.interrupt(thread).await.map_err(Failure::new),
        ("claude", _, _) => crate::claude::stop(node, resident),
        _ => Err(Failure::new(format!("{resident} is a session the World does not hold; stop it where it is open"))),
    }
}

/// Which machine the resident `id` lives on: this one when it lives here,
/// otherwise whichever says it does.
async fn locate(node: &Arc<Node>, id: &ResidentId) -> Result<MachineName, Failure> {
    if node.residents.about(id).is_some() {
        return Ok(node.machine());
    }
    if !node.connected_anywhere() {
        return Err(Failure::new(format!("no resident {id} lives on {}", node.machine())));
    }
    crate::colleagues::find_for(node, &Acting::Here, id).await
}

/// What the user chose for a session opened here, as this machine keeps it.
pub struct Opened {
    pub acting: Acting,
    pub works_on: Option<Written>,
    pub colleagues: Option<Written>,
    /// The agent's home here it is born in, when not the default one.
    pub home: Option<std::path::PathBuf>,
    /// The Codex home its colleagues use where they live, when not the
    /// default one there.
    pub colleagues_home: Option<std::path::PathBuf>,
    /// Where the files attached to its first words go, other than images,
    /// when the user chose.
    pub files_on: Option<Written>,
}
