//! One client's connection: the welcome, then its calls one at a time.

use std::sync::Arc;

use fastctx_net::link::CallFailure;
use fastctx_platform::ipc::Connection;
use fastctx_types::ipc::{ClientFrame, Failure, Hello, NodeFrame, PROTOCOL, Peer, Welcome};
use fastctx_types::remote::{RemoteCall, RemoteReply};
use fastctx_types::residents::{Activity, PendingState};
use fastctx_types::run::{RunReply, RunRequest};
use fastctx_types::town::{MachineWho, WhoReply, WhoScope, WorldWho};
use fastctx_types::world::{Node as NodeView, NodesReply, Revoked};
use fastctx_types::write::{WriteOp, WriteOutcome};
use fastctx_types::{Address, Call, MachineName, NoticeId, Reply, ResidentId, TownName};

use crate::Node;
use crate::town::caller::{self, Caller};

pub async fn serve(node: Arc<Node>, connection: Connection) {
    if let Err(error) = converse(&node, connection).await {
        node.log.line(format_args!("a connection ended in error: {error}"));
    }
}

async fn converse(node: &Arc<Node>, connection: Connection) -> std::io::Result<()> {
    let (mut reader, mut writer) = connection.split();
    let Some(hello) = reader.recv().await? else {
        return Ok(());
    };
    let hello: Hello = parse(&hello)?;
    if hello.protocol != PROTOCOL {
        let reason = format!(
            "this node is fastctx {} and speaks protocol {PROTOCOL}; the client is fastctx {} and speaks protocol {}",
            env!("CARGO_PKG_VERSION"),
            hello.version,
            hello.protocol
        );
        writer.send(&json(&Welcome::Refused { reason })).await?;
        return Ok(());
    }
    let welcome = Welcome::Accepted { machine: node.machine(), version: env!("CARGO_PKG_VERSION").to_string() };
    writer.send(&json(&welcome)).await?;
    // The residents that arrived on this connection are here until it
    // closes, however it closes; what was collected for them and not taken
    // is free to be handed over again then.
    let mut arrived = Arrived { node, residents: Vec::new(), collected: Vec::new() };
    while let Some(frame) = reader.recv().await? {
        let ClientFrame::Call { id, call } = parse(&frame)?;
        if let Call::Watch { resident, machine } = call {
            // A watch keeps the connection until one end leaves.
            let ended = match crate::talk::watch(node, resident, machine).await {
                Err(failure) => Err(failure),
                Ok(mut watched) => loop {
                    tokio::select! {
                        piece = watched.recv() => match piece {
                            Some(Ok(piece)) => writer.send(&json(&NodeFrame::Watched { id, watched: piece })).await?,
                            Some(Err(reason)) => break Err(Failure::new(reason)),
                            None => break Ok(Reply::Done),
                        },
                        // The client says nothing more on a watch; whatever
                        // comes, or the end, is it leaving.
                        _ = reader.recv() => return Ok(()),
                    }
                },
            };
            writer.send(&json(&NodeFrame::Reply { id, reply: ended })).await?;
            continue;
        }
        // A call is answered whole or not taken on: a node handing the
        // World to its successor answers what it has begun, then goes.
        let Some(_answering) = node.handoff.call() else {
            let reply = Err(Failure::new(format!(
                "the node on {} is handing over to its successor; ask again in a moment",
                node.machine()
            )));
            writer.send(&json(&NodeFrame::Reply { id, reply })).await?;
            continue;
        };
        let arriving = matches!(call, Call::Arrive { .. });
        let reply = match call {
            Call::Collect { resident } => collect(node, resident, &mut arrived.collected).await,
            Call::Taken { resident, ids } => taken(node, resident, ids, &mut arrived.collected).await,
            call => handle(node, &hello.peer, call).await,
        };
        if arriving && let Ok(Reply::Arrived(whereabouts)) = &reply {
            arrived.residents.push(whereabouts.resident.clone());
        }
        writer.send(&json(&NodeFrame::Reply { id, reply })).await?;
        writer.drain().await?;
    }
    Ok(())
}

struct Arrived<'a> {
    node: &'a Arc<Node>,
    residents: Vec<ResidentId>,
    collected: Vec<(ResidentId, NoticeId, Option<String>)>,
}

impl Drop for Arrived<'_> {
    fn drop(&mut self) {
        let mut taking = self.node.taking.lock().expect("never poisoned");
        for (resident, id, _) in &self.collected {
            taking.remove(&(resident.clone(), id.clone()));
        }
        drop(taking);
        for resident in self.node.residents.left(&self.residents) {
            crate::colleagues::left(self.node, resident);
        }
    }
}

/// What waits for a resident with no other way in, for the answer of one
/// of its World tools, and what it is to be told of where it lives.
async fn collect(
    node: &Arc<Node>,
    resident: ResidentId,
    collected: &mut Vec<(ResidentId, NoticeId, Option<String>)>,
) -> Result<Reply, Failure> {
    if native_delivery(node, &resident) {
        return notices_without_collection(node, &resident).await;
    }
    let speaking = node.residents.speaking(&resident).ok_or_else(|| Failure::new("unknown resident"))?;
    let _speaking = speaking.lock().await;
    if native_delivery(node, &resident) {
        return notices_without_collection(node, &resident).await;
    }
    let town = town_of(node, &resident)?;
    let directory = node.world.resident(town.as_ref(), &resident);
    let kept = blocking("reading what waits", move || crate::inbox::pending(&directory)).await?;
    let mut notices = Vec::new();
    {
        let mut taking = node.taking.lock().expect("never poisoned");
        for pending in kept.into_iter().filter(|pending| pending.state == PendingState::Waiting) {
            let town = town_of(node, &resident)?;
            let directory = node.world.resident(town.as_ref(), &resident);
            if crate::inbox::read(&directory, &pending.delivery.id)
                .map_err(|error| Failure::new(error.to_string()))?
                .is_none_or(|current| current.state != PendingState::Waiting)
            {
                continue;
            }
            let key = (resident.clone(), pending.delivery.id.clone());
            if taking.insert(key.clone()) {
                let turn = node
                    .residents
                    .now(&resident)
                    .filter(|now| now.activity == Activity::Busy)
                    .and_then(|now| now.turn)
                    .or_else(|| {
                        node.residents.about(&resident).and_then(|(about, _)| {
                            if about.world_started && about.host.agent.as_str() == "codex" {
                                about.session.and_then(|thread| node.codex.running_turn(&thread))
                            } else {
                                None
                            }
                        })
                    });
                collected.push((key.0, key.1, turn));
                notices.push(pending.delivery);
            }
        }
    }
    let notes = {
        let node = node.clone();
        blocking("reading the town", move || crate::town::notes::notes(&node, &resident)).await?
    };
    Ok(Reply::Notices { notices, notes })
}

fn native_delivery(node: &Node, resident: &ResidentId) -> bool {
    node.residents.about(resident).is_some_and(|(about, _)| {
        (about.world_started && about.host.agent.as_str() == "codex") || crate::claude::paired(node, resident)
    })
}

async fn notices_without_collection(node: &Arc<Node>, resident: &ResidentId) -> Result<Reply, Failure> {
    // Native subagents inherit their parent's MCP configuration. A tool
    // response cannot deliver the parent's words to the intended thread;
    // sessions with a native channel exclusively use that channel.
    let (node, resident) = (node.clone(), resident.clone());
    let notes = blocking("reading the town", move || crate::town::notes::notes(&node, &resident)).await?;
    Ok(Reply::Notices { notices: Vec::new(), notes })
}

/// What `collect` gave went out with an answer: it went into the turn the
/// resident runs, when the World sees its turns.
async fn taken(
    node: &Arc<Node>,
    resident: ResidentId,
    ids: Vec<NoticeId>,
    collected: &mut Vec<(ResidentId, NoticeId, Option<String>)>,
) -> Result<Reply, Failure> {
    let town = town_of(node, &resident)?;
    let directory = node.world.resident(town.as_ref(), &resident);
    let speaking = node.residents.speaking(&resident).ok_or_else(|| Failure::new("unknown resident"))?;
    let _speaking = speaking.lock().await;
    let mut by_turn: std::collections::BTreeMap<Option<String>, Vec<fastctx_types::residents::Pending>> =
        std::collections::BTreeMap::new();
    for id in &ids {
        let reserved = collected
            .iter()
            .find(|(owner, notice, _)| *owner == resident && notice == id)
            .ok_or_else(|| Failure::new("this connection did not collect that delivery"))?;
        if let Some(mut pending) =
            crate::inbox::read(&directory, id).map_err(|error| Failure::new(error.to_string()))?
        {
            if pending.working {
                pending.readers = node.residents.working_for(&resident);
                pending.working = false;
            }
            by_turn.entry(reserved.2.clone()).or_default().push(pending);
        }
    }
    for (turn, inputs) in by_turn {
        if let Some(turn) = turn {
            let mut handover =
                crate::delivery::begin(&directory, inputs).map_err(|error| Failure::new(error.to_string()))?;
            crate::speech::bind(node, &resident, &directory, &mut handover, &turn).await?;
        } else {
            // A host that exposes no turn identity can acknowledge transport
            // delivery, but cannot establish an answer obligation for a turn.
            for input in inputs {
                crate::speech::settle(&directory, &input.delivery.id).await?;
            }
        }
    }
    for id in ids {
        collected.retain(|(owner, notice, _)| *owner != resident || *notice != id);
        node.taking.lock().expect("never poisoned").remove(&(resident.clone(), id));
    }
    Ok(Reply::Done)
}

async fn handle(node: &Arc<Node>, peer: &Peer, call: Call) -> Result<Reply, Failure> {
    match call {
        Call::Arrive { host, hints } => {
            let claude = hints.inbox.is_some();
            let whereabouts = node.residents.arrive(&node.world, &node.machine(), host, hints)?;
            if claude {
                crate::claude::arrived(node, &whereabouts.resident).await;
            }
            Ok(Reply::Arrived(whereabouts))
        }
        Call::Run { resident, request, node: target } => {
            let town = town_of(node, &resident)?;
            run(node, resident, town, request, target).await.map(Reply::Run)
        }
        Call::Write { resident, op, node: target } => {
            let town = town_of(node, &resident)?;
            write(node, resident, town, op, target).await.map(|outcome| Reply::Written { outcome })
        }
        Call::JobChanged { job, owner, town } => node.jobs.changed(node, job, owner, town).await.map(|()| Reply::Done),
        Call::Saw { owner, town, seen } => {
            let (world, here) = (node.world.clone(), node.machine());
            blocking("recording what was seen", move || {
                crate::residents::record_seen(&world, &here, &owner, town.as_ref(), &seen)
            })
            .await
            .map(|()| Reply::Done)
        }
        Call::Who { all, town } => {
            let town = town.or_else(|| caller::of(node, peer).ok().and_then(|caller| caller.town(node)));
            who(node, all, town).await.map(Reply::Who)
        }
        Call::Look { ids } => Ok(Reply::Looked { looked: crate::look::look(node, ids).await }),
        Call::Talk { to, machine, text } => {
            users_voice(node, peer)?;
            crate::talk::talk(node, to, machine, text).await.map(|(to, landed)| Reply::Spoken { to, landed })
        }
        Call::Open { agent, town, node: target, text } => {
            users_voice(node, peer)?;
            crate::talk::open(node, agent, town, target, text)
                .await
                .map(|(resident, landed)| Reply::Opened { resident, landed })
        }
        Call::Stop { resident, machine } => {
            users_voice(node, peer)?;
            crate::talk::stop(node, resident, machine)
                .await
                .map(|(resident, stopped)| Reply::Stopped { resident, stopped })
        }
        Call::Watch { .. } => Err(Failure::new("a watch is answered as it goes, not at once")),
        Call::Collect { .. } | Call::Taken { .. } => Err(Failure::new("this is asked on a session's own connection")),
        Call::Ask { resident, label, prompt, node: target } => {
            let town = town_of(node, &resident)?;
            crate::colleagues::ask(node, resident, town, label, prompt, target)
                .await
                .map(|colleague| Reply::Asked { colleague })
        }
        Call::Message { resident, to, machine, text } => {
            let town = town_of(node, &resident)?;
            crate::colleagues::message(node, resident, town, to, machine, text)
                .await
                .map(|(to, landed)| Reply::Spoken { to, landed })
        }
        Call::Project(call) => {
            let caller = caller::of(node, peer)?;
            crate::town::project::call(node, &caller, call).await
        }
        Call::Journal(call) => {
            let caller = caller::of(node, peer)?;
            crate::town::journal::call(node, &caller, call).await
        }
        Call::Join { code, name } => {
            crate::world::join(node, &code, name).await.map(|(world, name)| Reply::Joined { world, name })
        }
        Call::Invite { name, expires } => {
            let link = member(node)?;
            link.invite(name, expires).await.map(|code| Reply::Code { code: code.to_text() }).map_err(Failure::new)
        }
        Call::Revoke { name, reason } => {
            let link = member(node)?;
            if name == link.membership().name {
                return Err(Failure::new("a machine leaves with `fastctx leave`, not by revoking itself"));
            }
            link.revoke(&name, &reason).await.map(|()| Reply::Done).map_err(Failure::new)
        }
        Call::Leave => crate::world::leave(node).await.map(|()| Reply::Done),
        Call::Copy { from, to } => crate::copy::copy(node, from, to).await.map(Reply::Copied),
        Call::Nodes => nodes(node).await.map(Reply::Nodes),
    }
}

/// Only the user speaks as the user: words from a resident's session, or
/// from a command one of them ran, are not the user's (`world.md`, 身份照实).
fn users_voice(node: &Node, peer: &Peer) -> Result<(), Failure> {
    match caller::of(node, peer)? {
        Caller::User => Ok(()),
        Caller::Resident(address) => Err(Failure::new(format!(
            "{} runs this; only the user speaks as the user. Residents speak to each other with message_colleague",
            address.resident
        ))),
        Caller::Session(agent) => {
            Err(Failure::new(format!("a {agent} session's own shell runs this; only the user speaks as the user")))
        }
    }
}

/// The town a resident of this machine lives in.
fn town_of(node: &Node, resident: &ResidentId) -> Result<Option<TownName>, Failure> {
    node.residents
        .town_of(resident)
        .ok_or_else(|| Failure::new(format!("no resident {resident} lives on {}", node.machine())))
}

fn member(node: &Node) -> Result<Arc<fastctx_net::link::Link>, Failure> {
    node.link().ok_or_else(|| {
        Failure::new(format!("{} is in no World with other machines; `fastctx join` joins one", node.machine()))
    })
}

/// Runs a script for a resident of this machine, here or on `target`, and
/// records the action in the resident's records here.
async fn run(
    node: &Arc<Node>,
    resident: ResidentId,
    town: Option<TownName>,
    request: RunRequest,
    target: Option<MachineName>,
) -> Result<RunReply, Failure> {
    let here = node.machine();
    let script = request.script.clone();
    let started_at = jiff::Timestamp::now();
    let reply = match target.filter(|target| *target != here) {
        None => {
            let owner = Address { machine: here.clone(), resident: resident.clone() };
            node.jobs.run(node, owner, town.clone(), request).await?.0
        }
        Some(target) => {
            let link = node.link().ok_or_else(|| {
                Failure::new(format!("there is no machine {target}: {here} is in no World with other machines"))
            })?;
            let call = RemoteCall::Run { resident: resident.clone(), town: town.clone(), request };
            match link.call(&target, call).await.map_err(|failure| Failure::new(called(&failure)))? {
                Ok(RemoteReply::Run(reply)) => reply,
                Ok(_) => {
                    return Err(Failure::new(format!("{target} answered something other than a run")));
                }
                Err(failure) => return Err(failure),
            }
        }
    };
    let action = crate::jobs::action(&reply, &script, started_at);
    let world = node.world.clone();
    blocking("recording the action", move || {
        crate::residents::record_action(&world, &resident, town.as_ref(), &action)
    })
    .await?;
    Ok(reply)
}

/// Writes files for a resident of this machine, here or on `target`, and
/// records the write in the resident's records here.
async fn write(
    node: &Arc<Node>,
    resident: ResidentId,
    town: Option<TownName>,
    op: WriteOp,
    target: Option<MachineName>,
) -> Result<WriteOutcome, Failure> {
    let here = node.machine();
    let started_at = jiff::Timestamp::now();
    let (outcome, machine) = match target.filter(|target| *target != here) {
        None => {
            let owner = Address { machine: here.clone(), resident: resident.clone() };
            let (world, machine, town, op) = (node.world.clone(), here.clone(), town.clone(), op.clone());
            let outcome =
                blocking("writing", move || crate::write::write_here(&world, &machine, &owner, town.as_ref(), op))
                    .await?;
            (outcome, here)
        }
        Some(target) => {
            let link = node.link().ok_or_else(|| {
                Failure::new(format!("there is no machine {target}: {here} is in no World with other machines"))
            })?;
            let call = RemoteCall::Write { resident: resident.clone(), town: town.clone(), op: op.clone() };
            match link.call(&target, call).await.map_err(|failure| Failure::new(called(&failure)))? {
                Ok(RemoteReply::Written { outcome }) => (outcome, target),
                Ok(_) => return Err(Failure::new(format!("{target} answered something other than a write"))),
                Err(failure) => return Err(failure),
            }
        }
    };
    let action = crate::write::action(&op, &outcome, &machine, started_at);
    let world = node.world.clone();
    blocking("recording the action", move || {
        crate::residents::record_action(&world, &resident, town.as_ref(), &action)
    })
    .await?;
    Ok(outcome)
}

/// A failed call, said for whoever reads the answer.
fn called(failure: &CallFailure) -> String {
    failure.to_string()
}

/// This machine's own answer to `fastctx w`: in `town`, or in every town
/// and none.
pub async fn who_here(node: &Node, all: bool, town: Option<TownName>) -> Result<WhoReply, Failure> {
    let everyone = node.residents.everyone();
    let former = node.residents.former();
    let (world, machine) = (node.world.clone(), node.machine());
    blocking("looking around", move || crate::who::who(&world, &machine, everyone, former, all, town.as_ref())).await
}

/// `fastctx w` in `town`, asking each machine its record names, or across
/// the World, asking every member; the connected ones are asked at once.
async fn who(node: &Arc<Node>, all: bool, town: Option<TownName>) -> Result<WorldWho, Failure> {
    let here = node.machine();
    let (scope, asked) = match &town {
        Some(name) => {
            crate::town::map::catch_up(node, name).await?;
            let (world, town) = (node.world.clone(), name.clone());
            let record = blocking("reading the town", move || crate::town::read::latest(&world, &town)).await?;
            let asked = record.as_ref().map(|record| record.machines.clone());
            (WhoScope::Town { town: name.clone(), record: record.map(Box::new) }, asked)
        }
        None => (WhoScope::World { towns: crate::town::project::list(node).await? }, None),
    };
    let asks_for = |machine: &MachineName| asked.as_ref().is_none_or(|asked| asked.contains(machine));
    let mut machines = Vec::new();
    if asks_for(&here) {
        machines.push(MachineWho::Answered { reply: who_here(node, all, town.clone()).await? });
    }
    if let Some(link) = node.link() {
        // The list of members and who is connected, as the hub has it now.
        let refreshed = link.refresh().await;
        let (table, presence) = link.members();
        let me = link.membership().name.clone();
        let mut asks = Vec::new();
        for member in table.members().filter(|member| member.name != me && asks_for(&member.name)) {
            if let Err(error) = &refreshed {
                machines.push(MachineWho::NoAnswer {
                    machine: member.name.clone(),
                    reason: format!("whether it is connected is not known: the hub could not be asked ({error})"),
                });
                continue;
            }
            let seen = presence.iter().find(|presence| presence.name == member.name).cloned();
            match seen {
                Some(presence) if presence.online => {
                    let (link, name, town) = (link.clone(), member.name.clone(), town.clone());
                    asks.push(tokio::spawn(async move {
                        match link.call(&name, RemoteCall::Who { all, town }).await {
                            Ok(Ok(RemoteReply::Who(reply))) => MachineWho::Answered { reply },
                            Ok(Ok(_)) => {
                                MachineWho::NoAnswer { machine: name, reason: "it answered something else".to_string() }
                            }
                            Ok(Err(failure)) => MachineWho::NoAnswer { machine: name, reason: failure.message },
                            Err(CallFailure::Offline { name, last_seen }) => {
                                MachineWho::Offline { machine: name, since: last_seen }
                            }
                            Err(failure) => MachineWho::NoAnswer { machine: name, reason: failure.to_string() },
                        }
                    }));
                }
                Some(presence) => {
                    machines.push(MachineWho::Offline { machine: member.name.clone(), since: presence.since })
                }
                None => machines.push(MachineWho::NoAnswer {
                    machine: member.name.clone(),
                    reason: "the hub did not say whether it is connected".to_string(),
                }),
            }
        }
        for ask in asks {
            machines.push(ask.await.map_err(|error| Failure::new(format!("asking around: {error}")))?);
        }
    }
    Ok(WorldWho { machine: here, scope, machines })
}

/// `fastctx nodes`: this machine's link, and each machine of the World with
/// what it says of itself.
async fn nodes(node: &Arc<Node>) -> Result<NodesReply, Failure> {
    let here = node.machine();
    let link = node.link();
    let mut machines = vec![NodeView {
        name: here.clone(),
        online: Some(true),
        since: None,
        facts: Ok(crate::world::facts(&here)),
        revoked: None,
    }];
    if let Some(link) = &link {
        let refreshed = link.refresh().await;
        let (table, presence) = link.members();
        let mut asks = Vec::new();
        for member in table.members().filter(|member| member.name != here) {
            let seen = presence.iter().find(|presence| presence.name == member.name).cloned();
            let (online, since) =
                seen.as_ref().map(|presence| (Some(presence.online), presence.since)).unwrap_or((None, None));
            let online = if refreshed.is_ok() { online } else { None };
            let (link, name) = (link.clone(), member.name.clone());
            let unknown = refreshed.as_ref().err().cloned();
            asks.push(tokio::spawn(async move {
                let facts = if online == Some(true) {
                    match link.call(&name, RemoteCall::Facts).await {
                        Ok(Ok(RemoteReply::Facts(facts))) => Ok(facts),
                        Ok(Ok(_)) => Err("it answered something else".to_string()),
                        Ok(Err(failure)) => Err(failure.message),
                        Err(failure) => Err(failure.to_string()),
                    }
                } else if online == Some(false) {
                    Err("it is offline".to_string())
                } else {
                    Err(match unknown {
                        Some(error) => format!("the hub could not be asked: {error}"),
                        None => "the hub did not say whether it is connected".to_string(),
                    })
                };
                NodeView { name, online, since, facts, revoked: None }
            }));
        }
        for (name, revocation) in table.revoked_members() {
            machines.push(NodeView {
                name: name.clone(),
                online: None,
                since: None,
                facts: Err("revoked".to_string()),
                revoked: Some(Revoked {
                    by: revocation.by.clone(),
                    at: revocation.at,
                    reason: revocation.reason.clone(),
                }),
            });
        }
        for ask in asks {
            machines.push(ask.await.map_err(|error| Failure::new(format!("asking around: {error}")))?);
        }
    }
    Ok(NodesReply {
        machine: here,
        world: link.as_ref().map(|link| link.membership().world.to_string()),
        link: crate::world::link_state(link.as_deref()),
        machines,
    })
}

async fn blocking<T: Send + 'static>(
    what: &str,
    work: impl FnOnce() -> std::io::Result<T> + Send + 'static,
) -> Result<T, Failure> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| Failure::new(format!("{what}: {error}")))?
        .map_err(|error| Failure::new(format!("{what}: {error}")))
}

fn parse<T: serde::de::DeserializeOwned>(frame: &[u8]) -> std::io::Result<T> {
    serde_json::from_slice(frame).map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
}

fn json(value: &impl serde::Serialize) -> Vec<u8> {
    serde_json::to_vec(value).expect("the World's types always serialize")
}
