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
use fastctx_net::link::Feed;
use fastctx_net::sign::Domain;
use fastctx_types::ipc::Failure;
use fastctx_types::remote::{RemoteCall, RemoteReply};
use fastctx_types::residents::{About, Activity, Delivery, Host, Landed, Notice, Now, Origin};
use fastctx_types::session::{SessionEvent, Speaker, Watched};
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
) -> Result<(Address, Landed), Failure> {
    let here = node.machine();
    let machine = match machine {
        Some(machine) => machine,
        None => locate(node, &to).await?,
    };
    if machine == here {
        let landed = talk_here(node, &here, &to, text).await?;
        return Ok((Address { machine: here, resident: to }, landed));
    }
    let link = link(node)?;
    let call = RemoteCall::Talk { to: to.clone(), text };
    match link.call(&machine, call).await.map_err(|failure| Failure::new(failure.to_string()))? {
        Ok(RemoteReply::Said { landed }) => Ok((Address { machine, resident: to }, landed)),
        Ok(_) => Err(Failure::new(format!("{machine} answered something other than a talk"))),
        Err(failure) => Err(failure),
    }
}

/// The user's words, typed on `typed_on`, to a resident of this machine.
pub async fn talk_here(
    node: &Arc<Node>,
    typed_on: &MachineName,
    to: &ResidentId,
    text: String,
) -> Result<Landed, Failure> {
    let notice = Notice::Words { from: Speaker::User { machine: typed_on.clone() }, text };
    let delivery = Delivery { id: speech::new_id("talk"), at: Timestamp::now(), notice };
    speech::tell_here(node, to, delivery, Owed::Nobody).await
}

/// Watches a resident on any machine. What it hands over comes on the
/// returned channel until the watch ends, with why when something went
/// wrong; dropping the channel ends the watch.
pub async fn watch(
    node: &Arc<Node>,
    resident: ResidentId,
    machine: Option<MachineName>,
) -> Result<mpsc::UnboundedReceiver<Result<Watched, String>>, Failure> {
    let here = node.machine();
    let machine = match machine {
        Some(machine) => machine,
        None => locate(node, &resident).await?,
    };
    let (sender, receiver) = mpsc::unbounded_channel();
    if machine == here {
        let (history, mut events) = watch_here(node, &resident).await?;
        let _ = sender.send(Ok(Watched::History { events: history }));
        tokio::spawn(async move {
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
    let link = link(node)?;
    let watch = u64::from_le_bytes(
        fastctx_net::sign::random::<8>().map_err(|error| Failure::new(format!("naming the watch: {error}")))?,
    );
    let (feeding, link_for_feed, from) = (sender.clone(), link.clone(), machine.clone());
    let feed: Feed = Box::new(move |mut stream: Stream| {
        Box::pin(async move {
            let mut n = 0;
            loop {
                let signed = tokio::select! {
                    signed = stream.hear::<fastctx_net::sign::Signed>() => signed?,
                    () = feeding.closed() => return Ok(()),
                };
                let piece: Piece = link_for_feed.unseal(&from, Domain::Event, &signed).await?;
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
    });
    let call = RemoteCall::Watch { resident, watch };
    tokio::spawn(async move {
        let ended = match link.call_with_stream(&machine, call, feed).await {
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
    let (about, _) = node
        .residents
        .about(resident)
        .ok_or_else(|| Failure::new(format!("no resident {resident} lives on {}", node.machine())))?;
    let events = node.watchers.watch(resident);
    let history = match &about.session {
        Some(thread) if about.world_started && about.host.agent.as_str() == "codex" => {
            node.codex.history(thread).await.map_err(Failure::new)?
        }
        _ => Vec::new(),
    };
    Ok((history, events))
}

/// Hands a watch of a resident of this machine to `from`, over `stream`,
/// until `from` closes it.
pub async fn serve_watch(
    node: &Arc<Node>,
    resident: ResidentId,
    watch: u64,
    mut stream: Stream,
) -> Result<RemoteReply, Failure> {
    let link = link(node)?;
    let (history, mut events) = watch_here(node, &resident).await?;
    let mut n = 0;
    let mut next = Some(Watched::History { events: history });
    loop {
        if let Some(watched) = next.take() {
            let signed = link.seal(Domain::Event, &Piece { watch, n, watched });
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

/// Opens a session for the user on this machine or on `target`.
pub async fn open(
    node: &Arc<Node>,
    agent: AgentName,
    town: Option<TownName>,
    target: Option<MachineName>,
    text: String,
) -> Result<(Address, Landed), Failure> {
    let here = node.machine();
    match target.filter(|target| *target != here) {
        None => {
            let (id, landed) = open_here(node, &here, agent, town, text).await?;
            Ok((Address { machine: here, resident: id }, landed))
        }
        Some(target) => {
            let link = link(node)?;
            let call = RemoteCall::Open { agent, town, text };
            match link.call(&target, call).await.map_err(|failure| Failure::new(failure.to_string()))? {
                Ok(RemoteReply::Opened { resident, landed }) => Ok((Address { machine: target, resident }, landed)),
                Ok(_) => Err(Failure::new(format!("{target} answered something other than an opened session"))),
                Err(failure) => Err(failure),
            }
        }
    }
}

/// Opens a session on this machine for the user typing on `typed_on`, with
/// their words as its first.
pub async fn open_here(
    node: &Arc<Node>,
    typed_on: &MachineName,
    agent: AgentName,
    town: Option<TownName>,
    text: String,
) -> Result<(ResidentId, Landed), Failure> {
    match agent.as_str() {
        "codex" => {}
        "claude" => return crate::claude::open_here(node, typed_on, town, text).await,
        _ => {
            return Err(Failure::new(format!(
                "the World opens codex and claude sessions; it cannot open {agent} ones"
            )));
        }
    }
    if let Some(town) = &town {
        crate::town::project::spans_here(node, town).await?;
    }
    let id = node.residents.new_id(&agent)?;
    let origin = Origin::User;
    let (spec, heard) = crate::colleagues::thread_spec(node, &id, &origin, town.as_ref()).await?;
    let (thread, model) = node.codex.start_thread(spec).await.map_err(Failure::new)?;
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
    };
    let now = Now {
        activity: Activity::Idle,
        since: born_at,
        turn: None,
        owed: Default::default(),
        answering: Vec::new(),
        trouble: None,
        desk: false,
    };
    node.residents
        .create(&node.world, about, town, now)
        .map_err(|error| Failure::new(format!("recording the session: {error}")))?;
    node.residents.set_heard(&id, heard);
    crate::codex::claim(node, &thread, &id).await;
    let landed = talk_here(node, typed_on, &id, text).await?;
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
    let link = link(node)?;
    let call = RemoteCall::Stop { resident: resident.clone() };
    match link.call(&machine, call).await.map_err(|failure| Failure::new(failure.to_string()))? {
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
    let (about, _) = node
        .residents
        .about(resident)
        .ok_or_else(|| Failure::new(format!("no resident {resident} lives on {}", node.machine())))?;
    match (about.host.agent.as_str(), about.world_started, &about.session) {
        ("codex", true, Some(thread)) => node.codex.interrupt(thread).await.map_err(Failure::new),
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
    crate::colleagues::find(node, id).await
}

fn link(node: &Node) -> Result<Arc<fastctx_net::link::Link>, Failure> {
    node.link().ok_or_else(|| Failure::new(format!("{} is in no World with other machines", node.machine())))
}
