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
    let envelope: serde_json::Value = parse(&hello)?;
    if envelope.get("maintenance").is_some() {
        let Some(_answering) = node.handoff.call() else { return Ok(()) };
        let response = crate::renewal::answer(node, parse(&hello)?)?;
        writer.send(&json(&response)).await?;
        return writer.drain().await;
    }
    let hello: Hello = parse(&hello)?;
    let legacy = crate::renewal_legacy::enabled(hello.protocol, &hello.peer);
    if hello.protocol != PROTOCOL && !legacy {
        let reason = format!(
            "this node is fastctx {} and speaks protocol {PROTOCOL}; the client is fastctx {} and speaks protocol {}",
            env!("CARGO_PKG_VERSION"),
            hello.version,
            hello.protocol
        );
        let mismatch = Some(fastctx_types::renewal::Mismatch {
            node_version: env!("CARGO_PKG_VERSION").into(),
            node_protocol: PROTOCOL,
            client_version: hello.version,
            client_protocol: hello.protocol,
        });
        writer.send(&json(&Welcome::Refused { reason, mismatch })).await?;
        return Ok(());
    }
    let welcome = Welcome::Accepted { machine: node.machine(), version: env!("CARGO_PKG_VERSION").to_string() };
    writer.send(&json(&welcome)).await?;
    // The residents that arrived on this connection are here until it
    // closes, however it closes; what was collected for them and not taken
    // is free to be handed over again then.
    let mut arrived = Arrived { node, residents: Vec::new(), collected: Vec::new(), notes: Vec::new() };
    while let Some(frame) = reader.recv().await? {
        let ClientFrame::Call { id, call } = match parse(&frame) {
            Ok(call) => call,
            Err(error) => {
                let envelope: serde_json::Value = parse(&frame)?;
                let Some(id) = envelope.get("id").and_then(serde_json::Value::as_u64) else {
                    return Err(error);
                };
                let reply = Err(Failure::new(format!("the request was not executed: {error}")));
                writer.send(&crate::renewal_legacy::frame(hello.protocol, id, &reply)).await?;
                writer.drain().await?;
                continue;
            }
        };
        if legacy && !crate::renewal_legacy::call(hello.protocol, &hello.peer, &call) {
            writer
                .send(&json(&NodeFrame::Reply {
                    id,
                    reply: Err(Failure::new("this call is outside the frozen protocol-5/6 tool and job contract")),
                }))
                .await?;
            return writer.drain().await;
        }
        if let Call::Watch { resident, machine, composer } = call {
            // A watch keeps the connection until one end leaves.
            let ended = match crate::talk::watch(node, resident, machine, composer).await {
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
        if let Call::Wait { jobs } = call {
            // A wait keeps the connection until the jobs end; its leaving
            // hands back the ends the wait held (`waiting`).
            let caller = match &hello.peer {
                Peer::Command { resident, .. } => resident.clone(),
                _ => None,
            };
            let waited = tokio::select! {
                waited = crate::waiting::wait(node, caller, jobs) => waited,
                // The client says nothing more on a wait.
                _ = reader.recv() => return Ok(()),
            };
            writer.send(&json(&NodeFrame::Reply { id, reply: waited })).await?;
            continue;
        }
        if let Call::On { resident, town, request } = call {
            // Only the World's shell of that resident sends its `on`s: the
            // command speaks as the resident its job belongs to.
            let ended = match &hello.peer {
                Peer::Command { resident: Some(speaker), .. } if *speaker == resident => {
                    let origin = fastctx_types::on::OnOrigin { resident, town };
                    match crate::on::send(node, origin, request).await {
                        Err(failure) => Err(failure),
                        Ok(mut pieces) => loop {
                            tokio::select! {
                                piece = pieces.recv() => match piece {
                                    Some(Ok(piece)) => writer.send(&json(&NodeFrame::On { id, piece })).await?,
                                    Some(Err(failure)) => break Err(failure),
                                    None => break Ok(Reply::Done),
                                },
                                // The client says nothing more on an `on`;
                                // its leaving stops the job.
                                _ = reader.recv() => return Ok(()),
                            }
                        },
                    }
                }
                _ => Err(Failure::new("on works only inside a script run by the run tool")),
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
        if let Call::ReadFile { machine, request } = call {
            if let Err(failure) = users_voice(node, &hello.peer) {
                writer.send(&json(&NodeFrame::Reply { id, reply: Err(failure) })).await?;
                return writer.drain().await;
            }
            return crate::file_read::serve_local(node, machine, request, id, &mut reader, &mut writer).await;
        }
        if let Call::Upload { target, node: machine, file, composer } = call {
            let reply = match users_voice(node, &hello.peer) {
                Ok(()) => crate::attachments::upload(node, target, machine, file, composer, reader)
                    .await
                    .map(crate::attachments::reply),
                Err(error) => Err(error),
            };
            writer.send(&json(&NodeFrame::Reply { id, reply })).await?;
            writer.drain().await?;
            return Ok(());
        }
        if let Some(removed) = arrived.residents.iter().find(|id| node.residents.removed(id).is_some()) {
            let reply = Err::<Reply, _>(node.residents.active(removed).expect_err("removed resident"));
            writer.send(&json(&NodeFrame::Reply { id, reply })).await?;
            writer.drain().await?;
            continue;
        }
        let arriving = matches!(&call, Call::Arrive { host, hints } if host.agent.as_str() != "claude" && hints.tool_use_id.is_none());
        let mut reply = match call {
            Call::Collect { resident, answering } => {
                collect(node, resident, answering, &mut arrived.collected, &mut arrived.notes).await
            }
            Call::Taken { resident, ids } => {
                taken(node, resident, ids, &mut arrived.collected, &mut arrived.notes).await
            }
            call => handle(node, &hello.peer, call).await,
        };
        if legacy && let Ok(reply) = &mut reply {
            crate::renewal_legacy::reply(hello.protocol, reply);
        }
        if arriving && let Ok(Reply::Arrived(whereabouts)) = &reply {
            arrived.residents.push(whereabouts.resident.clone());
        }
        writer.send(&crate::renewal_legacy::frame(hello.protocol, id, &reply)).await?;
        writer.drain().await?;
    }
    Ok(())
}

struct Arrived<'a> {
    node: &'a Arc<Node>,
    residents: Vec<ResidentId>,
    collected: Vec<(ResidentId, NoticeId, Option<String>)>,
    notes: Vec<(ResidentId, crate::residents::Heard)>,
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
    answering: Option<fastctx_types::JobId>,
    collected: &mut Vec<(ResidentId, NoticeId, Option<String>)>,
    collected_notes: &mut Vec<(ResidentId, crate::residents::Heard)>,
) -> Result<Reply, Failure> {
    if native_delivery(node, &resident) {
        return collect_notes(node, resident, answering, Vec::new(), collected_notes).await;
    }
    let speaking = node.residents.speaking(&resident).ok_or_else(|| Failure::new("unknown resident"))?;
    let _speaking = speaking.lock().await;
    if native_delivery(node, &resident) {
        return collect_notes(node, resident, answering, Vec::new(), collected_notes).await;
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
    collect_notes(node, resident, answering, notices, collected_notes).await
}

fn native_delivery(node: &Node, resident: &ResidentId) -> bool {
    // Native subagents inherit their parent's MCP configuration. A tool
    // response cannot deliver the parent's words to the intended thread;
    // sessions with a native channel exclusively use that channel.
    node.residents.about(resident).is_some_and(|(about, _)| {
        (about.world_started && about.host.agent.as_str() == "codex") || crate::claude::paired(node, resident)
    })
}

async fn collect_notes(
    node: &Arc<Node>,
    resident: ResidentId,
    answering: Option<fastctx_types::JobId>,
    notices: Vec<fastctx_types::residents::Delivery>,
    collected_notes: &mut Vec<(ResidentId, crate::residents::Heard)>,
) -> Result<Reply, Failure> {
    let (node, id) = (node.clone(), resident.clone());
    let (notes, heard) = blocking("reading the town", move || {
        let (mut notes, heard) = crate::town::notes::notes(&node, &id)?;
        notes.extend(crate::jobs::still_running(&node.world, &node.machine(), &id, answering.as_ref())?);
        Ok((notes, heard))
    })
    .await?;
    if let Some(heard) = heard {
        collected_notes.push((resident, heard));
    }
    Ok(Reply::Notices { notices, notes })
}

/// What `collect` gave went out with an answer: it went into the turn the
/// resident runs, when the World sees its turns.
async fn taken(
    node: &Arc<Node>,
    resident: ResidentId,
    ids: Vec<NoticeId>,
    collected: &mut Vec<(ResidentId, NoticeId, Option<String>)>,
    collected_notes: &mut Vec<(ResidentId, crate::residents::Heard)>,
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
    collected_notes.retain(|(owner, heard)| {
        if *owner != resident {
            return true;
        }
        node.residents.acknowledge_heard(owner, heard.clone());
        false
    });
    Ok(Reply::Done)
}

pub(crate) async fn handle(node: &Arc<Node>, peer: &Peer, call: Call) -> Result<Reply, Failure> {
    if let Ok(Caller::Resident(address)) = caller::of(node, peer)
        && address.machine == node.machine()
    {
        node.residents.active(&address.resident)?;
    }
    match call {
        Call::Arrive { host, hints } => {
            if host.agent.as_str() == "claude" || hints.tool_use_id.is_some() {
                let id = hints.tool_use_id.as_deref().filter(|id| !id.is_empty()).ok_or_else(|| {
                    Failure::new("Claude did not supply a tool use id; its session cannot be identified")
                })?;
                return crate::claude::identify(node, id).await.map(Reply::Arrived);
            }
            let whereabouts = node.residents.arrive(&node.world, &node.machine(), host, hints)?;
            Ok(Reply::Arrived(whereabouts))
        }
        Call::Run { resident, request, node: target } => {
            let town = town_of(node, &resident)?;
            run(node, resident, town, request, target).await.map(Reply::Run)
        }
        Call::Write { resident: Some(resident), op, node: target, .. } => {
            let town = town_of(node, &resident)?;
            write(node, resident, town, op, target).await.map(|outcome| Reply::Written { outcome })
        }
        Call::Write { resident: None, op, node: target, town } => {
            if target.is_some() {
                return Err(Failure::new(
                    "write commands run on this machine; run the command on the destination machine",
                ));
            }
            crate::write_command::write(node, peer, town, op).await.map(|outcome| Reply::Written { outcome })
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
        Call::Who { all, town, since } => {
            let caller = caller::of(node, peer)?;
            let town = town.or_else(|| caller.town(node));
            let you = match caller {
                caller::Caller::Resident(address) => Some(address),
                _ => None,
            };
            let since = match since {
                Some(at) => Some(at),
                None => match &you {
                    Some(address) if address.machine == node.machine() => {
                        let world = node.world.clone();
                        let id = address.resident.clone();
                        let town = node.residents.town_of(&id).flatten();
                        blocking("reading the previous turn", move || {
                            crate::activity::last_turn(&world.resident(town.as_ref(), &id))
                        })
                        .await?
                    }
                    _ => None,
                },
            };
            let mut reply = who(node, all, town, since).await?;
            reply.you = you;
            Ok(Reply::Who(reply))
        }
        Call::Since { since, town } => crate::since::query(node, since, town).await.map(Reply::Since),
        Call::Look { ids } => Ok(Reply::Looked { looked: crate::look::look(node, ids).await }),
        Call::StopJob { job, machine } => {
            let by = match caller::of(node, peer)? {
                Caller::Resident(address) => address.tag(),
                Caller::User => "the user".to_string(),
                Caller::Session(agent) => format!("a {agent} session"),
                Caller::Page(page) => format!("the page {}", page.page),
            };
            crate::stop::stop(node, by, job, machine).await
        }
        Call::Talk { to, machine, text, attachments } => {
            users_voice(node, peer)?;
            crate::talk::talk(node, to, machine, text, attachments)
                .await
                .map(|(to, landed)| Reply::Spoken { to, landed })
        }
        Call::Agents { machine } => crate::agents::query(node, machine).await.map(Reply::Agents),
        Call::ClaudeConsent { machine, accept } => {
            users_voice(node, peer)?;
            crate::consent::call(node, machine.clone(), accept).await.map(|(accepted, changed)| Reply::ClaudeConsent {
                machine,
                accepted,
                changed,
            })
        }
        Call::FindFiles { machine, request } => {
            crate::file_search::call(node, machine, request).await.map(Reply::FilesFound)
        }
        Call::Open { agent, town, node: target, text, model, reasoning_effort, attachments } => {
            users_voice(node, peer)?;
            let options = fastctx_types::agents::SessionOptions { model, reasoning_effort };
            crate::talk::open(node, agent, town, target, text, options, attachments)
                .await
                .map(|(resident, landed)| Reply::Opened { resident, landed })
        }
        Call::Stop { resident, machine } => {
            users_voice(node, peer)?;
            crate::talk::stop(node, resident, machine)
                .await
                .map(|(resident, stopped)| Reply::Stopped { resident, stopped })
        }
        Call::Manage { resident, machine, action } => {
            users_voice(node, peer)?;
            crate::management::manage(node, resident, machine, action)
                .await
                .map(|(resident, managed)| Reply::Managed { resident, managed })
        }
        Call::Control { resident, machine, control } => {
            users_voice(node, peer)?;
            crate::control::control(node, resident, machine, control)
                .await
                .map(|(resident, controlled)| Reply::Controlled { resident, controlled })
        }
        Call::Composer { resident, machine, request } => {
            users_voice(node, peer)?;
            crate::composer::request(node, resident, machine, request)
                .await
                .map(|updated| Reply::Composer(Box::new(updated)))
        }
        Call::Offers { resident, machine } => crate::control::offers(node, resident, machine)
            .await
            .map(|(resident, offers)| Reply::Offers { resident, offers: Box::new(offers) }),
        Call::Attach { resident } => {
            users_voice(node, peer)?;
            crate::attach::command(node, &resident).await.map(Reply::Attachable)
        }
        Call::Watch { .. } => Err(Failure::new("a watch is answered as it goes, not at once")),
        Call::On { .. } => Err(Failure::new("an on is answered as it goes, not at once")),
        Call::ReadFile { .. } => Err(Failure::new("a file read is answered as a byte stream")),
        Call::Upload { .. } => Err(Failure::new("an upload requires its byte stream on a dedicated connection")),
        Call::Collect { .. } | Call::Taken { .. } => Err(Failure::new("this is asked on a session's own connection")),
        Call::Wait { .. } => Err(Failure::new("a wait is asked on a command's own connection")),
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
        Call::Page(call) => {
            let caller = caller::of(node, peer)?;
            crate::pages::call(node, &caller, &node.machine(), call).await
        }
        // Who belongs to the World is the user's to decide: a resident that
        // could invite or revoke a machine could hand the World to anyone.
        Call::Join { code, name } => {
            users_voice(node, peer)?;
            crate::world::join(node, &code, name).await.map(|(world, name, already)| Reply::Joined {
                world,
                name,
                already,
            })
        }
        Call::Invite { name, expires, device, scope } => {
            users_voice(node, peer)?;
            let link = member(node)?;
            let kind = if device { fastctx_net::trust::Kind::Device } else { fastctx_net::trust::Kind::Machine };
            let scope =
                scope.map(|scope| crate::ledger::names::by_id(node, scope)).transpose().map_err(Failure::new)?;
            link.invite(name, expires, kind, scope)
                .await
                .map(|code| Reply::Code { code: code.to_text() })
                .map_err(Failure::new)
        }
        Call::InviteServer { name, expires, scope } => {
            users_voice(node, peer)?;
            crate::server_invitation::invite(node, name, expires, scope)
                .await
                .map(|command| Reply::ServerInvitation { command })
        }
        Call::Grant { member: name, scope, manage } => {
            users_voice(node, peer)?;
            let link = member(node)?;
            let scope = crate::ledger::names::by_id(node, scope).map_err(Failure::new)?;
            link.grant(&name, scope, manage).await.map(|()| Reply::Done).map_err(Failure::new)
        }
        Call::Route { member: name, via } => {
            users_voice(node, peer)?;
            let link = member(node)?;
            let via = via
                .into_iter()
                .map(|upstream| match upstream {
                    fastctx_types::world::Upstream::Hub => fastctx_net::tree::Upstream::Hub,
                    fastctx_types::world::Upstream::Relay { machine } => fastctx_net::tree::Upstream::Relay { machine },
                })
                .collect();
            link.route(&name, via).await.map(|()| Reply::Done).map_err(Failure::new)
        }
        Call::SetRelay { machine, relay } => {
            users_voice(node, peer)?;
            let link = member(node)?;
            if machine == link.membership().name {
                return crate::relay::set(node, relay).await.map(|relays_at| Reply::Relaying { relays_at });
            }
            match link.call(&machine, fastctx_types::remote::RemoteCall::SetRelay { relay }).await {
                Ok(Ok(fastctx_types::remote::RemoteReply::Relaying { relays_at })) => Ok(Reply::Relaying { relays_at }),
                Ok(Ok(_)) => Err(Failure::new(format!("{machine} answered something else"))),
                Ok(Err(failure)) => Err(failure),
                Err(failure) => Err(Failure::new(failure.to_string())),
            }
        }
        Call::Seen { member: name } => {
            users_voice(node, peer)?;
            crate::admitted::shown(node, &name)
                .map(|()| Reply::Done)
                .map_err(|error| Failure::new(format!("recording that {name} was seen: {error}")))
        }
        Call::Revoke { name, reason } => {
            users_voice(node, peer)?;
            let link = member(node)?;
            if name == link.membership().name {
                return Err(Failure::new("a machine leaves with `fastctx leave`, not by revoking itself"));
            }
            link.revoke(&name, &reason).await.map(|()| Reply::Done).map_err(Failure::new)
        }
        Call::Leave => {
            users_voice(node, peer)?;
            crate::world::leave(node).await.map(|()| Reply::Done)
        }
        Call::HandOver => {
            users_voice(node, peer)?;
            crate::handoff::hand_over(node.clone());
            Ok(Reply::Done)
        }
        Call::Copy { from, to } => crate::copy::copy(node, from, to).await.map(Reply::Copied),
        Call::Nodes => nodes(node).await.map(Reply::Nodes),
        Call::Machines => Ok(machines(node)),
    }
}

/// Only the user speaks as the user: words from a resident's session, or
/// from a command one of them ran, are not the user's (`world.md`, 身份照实).
fn users_voice(node: &Node, peer: &Peer) -> Result<(), Failure> {
    match caller::of(node, peer)? {
        Caller::User => Ok(()),
        Caller::Page(page) => Err(Failure::new(format!("{page} runs this; only the user speaks as the user"))),
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
    node.residents.active(resident)?;
    node.residents
        .town_of(resident)
        .ok_or_else(|| Failure::new(format!("no resident {resident} lives on {}", node.machine())))
}

fn member(node: &Node) -> Result<Arc<fastctx_net::link::Link>, Failure> {
    node.link().ok_or_else(|| Failure::new(format!("{} is not connected to any other machine", node.machine())))
}

/// Runs a script for a resident of this machine and records the action in
/// the resident's records. Other machines are reached with `on` inside the
/// script; a session listed before `on` may still name one, and is told so.
async fn run(
    node: &Arc<Node>,
    resident: ResidentId,
    town: Option<TownName>,
    request: RunRequest,
    target: Option<MachineName>,
) -> Result<RunReply, Failure> {
    let here = node.machine();
    if let Some(target) = target.filter(|target| *target != here) {
        return Err(Failure::new(format!(
            "run goes to another machine only through `on`: write `on {target}` in the script to run part of it there"
        )));
    }
    let script = request.script.clone();
    let started_at = jiff::Timestamp::now();
    let owner = Address { machine: here.clone(), resident: resident.clone() };
    let reply = node.jobs.run(node, owner.into(), town.clone(), request).await?.0;
    let action = crate::jobs::action(&reply, &script, started_at);
    let world = node.world.clone();
    blocking("recording the action", move || {
        crate::residents::record_action(&world, &resident, town.as_ref(), &action)
    })
    .await?;
    Ok(reply)
}

/// Writes files for a resident of this machine, from a writing tool a
/// session listed before writing became commands of the shell, and records
/// the write in the resident's records. Files elsewhere are written there.
async fn write(
    node: &Arc<Node>,
    resident: ResidentId,
    town: Option<TownName>,
    op: WriteOp,
    target: Option<MachineName>,
) -> Result<WriteOutcome, Failure> {
    let here = node.machine();
    if let Some(target) = target.filter(|target| *target != here) {
        return Err(Failure::new(format!(
            "files on {target} are written there: `on {target} edit PATH` or `on {target} write PATH` in the run tool"
        )));
    }
    let started_at = jiff::Timestamp::now();
    let owner = Address { machine: here.clone(), resident: resident.clone() };
    let (world, machine, writing, written) = (node.world.clone(), here.clone(), town.clone(), op.clone());
    let outcome =
        blocking("writing", move || crate::write::write_here(&world, &machine, &owner, writing.as_ref(), written))
            .await?;
    let action = crate::write::action(&op, &outcome, &here, started_at);
    let world = node.world.clone();
    blocking("recording the action", move || {
        crate::residents::record_action(&world, &resident, town.as_ref(), &action)
    })
    .await?;
    Ok(outcome)
}

/// This machine's own answer to `fastctx w`: in `town`, or in every town
/// and none.
pub async fn who_here(
    node: &Node,
    all: bool,
    town: Option<TownName>,
    since: Option<jiff::Timestamp>,
) -> Result<WhoReply, Failure> {
    let everyone = node.residents.everyone();
    let former = node.residents.former();
    let (world, machine) = (node.world.clone(), node.machine());
    let place = town.clone();
    let mut reply =
        blocking("looking around", move || crate::who::who(&world, &machine, everyone, former, all, place.as_ref()))
            .await?;
    for resident in &mut reply.residents {
        resident.tools = node.watchers.tools(&resident.about.id);
        if let Some(now) = &mut resident.now {
            // Persisted occupancy predates a restarted node. Only the desk's
            // current host observations establish who holds it now.
            now.desk = node.desk.held_by(&resident.about.id);
            if !now.desk {
                now.desk_since = None;
            }
        }
    }
    if let Some(since) = since {
        reply.history = Some(crate::since::here(node, since, town).await?);
    }
    Ok(reply)
}

/// `fastctx w` in `town`, asking each machine its record names, or across
/// the World, asking every member; the connected ones are asked at once.
async fn who(
    node: &Arc<Node>,
    all: bool,
    town: Option<TownName>,
    since: Option<jiff::Timestamp>,
) -> Result<WorldWho, Failure> {
    let here = node.machine();
    let (scope, asked) = match &town {
        Some(name) => {
            crate::town::map::query(node, name).await?;
            let (world, town) = (node.world.clone(), name.clone());
            let record = blocking("reading the town", move || crate::town::read::latest(&world, &town)).await?;
            let asked = record.as_ref().map(|record| record.machines.clone());
            (WhoScope::Town { town: name.clone(), record: record.map(Box::new) }, asked)
        }
        None => (WhoScope::World { towns: crate::ledger::query::list(node).await? }, None),
    };
    let asks_for = |machine: &MachineName| asked.as_ref().is_none_or(|asked| asked.contains(machine));
    let mut machines = Vec::new();
    if asks_for(&here) {
        machines.push(match who_here(node, all, town.clone(), since).await {
            Ok(reply) => MachineWho::Answered { reply },
            Err(error) => MachineWho::NoAnswer { machine: here.clone(), reason: error.message },
        });
    }
    let mut presence_known = false;
    if let Some(link) = node.link() {
        // The list of members and who is connected, as the hub has it now.
        let refreshed = link.refresh().await;
        presence_known = refreshed.is_ok();
        let (table, presence) = link.members();
        let me = link.membership().name.clone();
        let mut asks = Vec::new();
        for member in table.machines().filter(|member| {
            member.name != me && asks_for(&member.name) && crate::admission::reaches(node, &table, &member.name)
        }) {
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
                    asks.push((
                        name.clone(),
                        tokio::spawn(async move {
                            match link.call(&name, RemoteCall::Who { all, town, since }).await {
                                Ok(Ok(RemoteReply::Who(reply))) if reply.machine == name => {
                                    MachineWho::Answered { reply }
                                }
                                Ok(Ok(_)) => MachineWho::NoAnswer {
                                    machine: name,
                                    reason: "it answered something else".to_string(),
                                },
                                Ok(Err(failure)) => MachineWho::NoAnswer { machine: name, reason: failure.message },
                                Err(CallFailure::Offline { name, last_seen }) => {
                                    MachineWho::Offline { machine: name, since: last_seen }
                                }
                                Err(failure) => MachineWho::NoAnswer { machine: name, reason: failure.to_string() },
                            }
                        }),
                    ));
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
        for (name, ask) in asks {
            machines.push(match ask.await {
                Ok(answer) => answer,
                Err(error) => MachineWho::NoAnswer { machine: name, reason: format!("asking around: {error}") },
            });
        }
    }
    if let Some(asked) = asked {
        for name in asked {
            if !machines.iter().any(|answer| match answer {
                MachineWho::Answered { reply } => reply.machine == name,
                MachineWho::Offline { machine, .. } | MachineWho::NoAnswer { machine, .. } => *machine == name,
            }) {
                machines.push(MachineWho::NoAnswer {
                    machine: name,
                    reason: "the town names this machine, but it could not be reached through this node".into(),
                });
            }
        }
    }
    let mut reply = WorldWho { machine: here, scope, machines, you: None, history: None };
    if let Some(since) = since {
        reply.collect_history(since, town.clone());
        crate::since::presence_for_who(node, &mut reply, presence_known).await?;
    }
    Ok(reply)
}

/// `fastctx nodes`: this machine's link, and each member of the World its
/// scope covers with what it says of itself and where it stands.
/// This machine and the other machines its scope reaches, from the members
/// table as it stands, asking none of them.
fn machines(node: &Arc<Node>) -> Reply {
    let here = node.machine();
    let others = node
        .link()
        .map(|link| {
            let (table, _) = link.members();
            table
                .machines()
                .filter(|member| member.name != here && crate::admission::reaches(node, &table, &member.name))
                .map(|member| member.name.clone())
                .collect()
        })
        .unwrap_or_default();
    Reply::Machines { here, others }
}

async fn nodes(node: &Arc<Node>) -> Result<NodesReply, Failure> {
    let here = node.machine();
    let link = node.link();
    let standing = |member: &fastctx_net::trust::Member| fastctx_types::world::Standing {
        scope: crate::ledger::names::by_name(node, &member.scope),
        manage: member.manage,
        invited_by: member.invited_by.clone(),
        delegated: member.delegated,
        sealing: member.seal.is_some(),
    };
    let mut machines = vec![NodeView {
        name: here.clone(),
        online: Some(true),
        since: None,
        facts: Ok(crate::world::facts(&here)),
        revoked: None,
        device: false,
        standing: link.as_ref().and_then(|link| link.me()).as_ref().map(standing),
        place: link.as_ref().and_then(|link| fastctx_net::tree::place(&link.table(), &here)),
    }];
    if let Some(link) = &link {
        let refreshed = link.refresh().await;
        let (table, presence) = link.members();
        let mut asks = Vec::new();
        for member in table
            .members()
            .filter(|member| member.name != here && crate::admission::reaches(node, &table, &member.name))
        {
            let seen = presence.iter().find(|presence| presence.name == member.name).cloned();
            let device = !member.kind.is_machine();
            let (online, since) =
                seen.as_ref().map(|presence| (Some(presence.online), presence.since)).unwrap_or((None, None));
            let online = if refreshed.is_ok() { online } else { None };
            let (link, name, standing) = (link.clone(), member.name.clone(), Some(standing(member)));
            let place = fastctx_net::tree::place(&table, &name);
            let unknown = refreshed.as_ref().err().cloned();
            asks.push(tokio::spawn(async move {
                // A device is listed as such and never asked: nothing runs
                // on it that could answer.
                let facts = if device {
                    Err("a device".to_string())
                } else if online == Some(true) {
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
                NodeView { name, online, since, facts, revoked: None, device, standing, place }
            }));
        }
        for (name, revocation) in table.revoked_members() {
            machines.push(NodeView {
                name: name.clone(),
                online: None,
                since: None,
                facts: Err("revoked".to_string()),
                device: false,
                revoked: Some(Revoked {
                    by: revocation.by.clone(),
                    at: revocation.at,
                    reason: revocation.reason.clone(),
                }),
                standing: None,
                place: None,
            });
        }
        for ask in asks {
            machines.push(ask.await.map_err(|error| Failure::new(format!("asking around: {error}")))?);
        }
    }
    let joined = crate::admitted::fresh(node)
        .into_iter()
        .map(|admitted| fastctx_types::world::Joined {
            name: admitted.name,
            device: admitted.device,
            noticed_at: admitted.noticed_at,
        })
        .collect();
    Ok(NodesReply {
        machine: here,
        world: link.as_ref().map(|link| link.membership().world.to_string()),
        link: crate::world::link_state(link.as_deref()),
        machines,
        joined,
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
