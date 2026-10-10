//! One client's connection: the welcome, then its calls one at a time.

use std::sync::Arc;

use fastctx_platform::ipc::Connection;
use fastctx_types::ipc::{ClientFrame, Failure, Hello, NodeFrame, PROTOCOL, Peer, Welcome};
use fastctx_types::remote::{RemoteCall, RemoteReply};
use fastctx_types::residents::{Activity, PendingState};
use fastctx_types::run::{RunReply, RunRequest};
use fastctx_types::town::{MachineWho, WhoReply, WhoScope, WorldWho};
use fastctx_types::world::{Node as NodeView, NodesReply};
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
    let welcome = Welcome::Accepted {
        machine: node.machine(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        node_lifetime: Some(Box::new(node.lifetime.clone())),
    };
    writer.send(&json(&welcome)).await?;
    // The residents that arrived on this connection are here until it
    // closes, however it closes; what was collected for them and not taken
    // is free to be handed over again then.
    let mut arrived = Arrived { node, residents: Vec::new(), collected: Vec::new(), notes: Vec::new(), completed: None };
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
                    reply: Err(Failure::new("this call is outside the frozen protocol-5/6/7 tool and job contract")),
                }))
                .await?;
            return writer.drain().await;
        }
        if let Call::Watch { resident, machine, composer } = call {
            // A watch keeps the connection until one end leaves.
            let opening = tokio::select! {
                result = crate::talk::watch(node, resident, machine, composer) => result,
                _ = reader.recv() => return Ok(()),
            };
            let ended = match opening {
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
        if let Call::WatchTools { resident: watched } = call {
            // A session's MCP server lists its tools again when told; it
            // keeps this connection for as long as it serves.
            let mut changes = node.tools.subscribe();
            loop {
                tokio::select! {
                    change = changes.recv() => {
                        let tell = match change {
                            Ok(Some(resident)) => watched.as_ref() == Some(&resident),
                            Ok(None) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => true,
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => return Ok(()),
                        };
                        if tell {
                            writer.send(&json(&NodeFrame::ToolsChanged { id })).await?;
                        }
                    }
                    _ = reader.recv() => return Ok(()),
                }
            }
        }
        if let Call::Wait { jobs, receipt, parent } = call {
            // A wait keeps the connection until the jobs end; its leaving
            // hands back the ends the wait held (`waiting`).
            let caller = match caller::of(node, &hello.peer) {
                Ok(Caller::Resident(resident)) => Some(resident),
                Ok(_) => None,
                Err(error) => {
                    writer.send(&json(&NodeFrame::Reply { id, reply: Err(error) })).await?;
                    continue;
                }
            };
            let waited = tokio::select! {
                waited = crate::waiting::wait(node, caller, jobs, parent) => waited,
                // The client says nothing more on a wait.
                _ = reader.recv() => return Ok(()),
            };
            match waited {
                Err(error) => writer.send(&json(&NodeFrame::Reply { id, reply: Err(error) })).await?,
                Ok(ready) => {
                    writer.send(&json(&NodeFrame::Reply { id, reply: Ok(ready.reply.clone()) })).await?;
                    writer.drain().await?;
                    if receipt {
                        let Some(frame) = reader.recv().await? else { return Ok(()) };
                        let word: String = parse(&frame)?;
                        if word != fastctx_types::on::TAKEN {
                            return Err(std::io::Error::other("the wait client did not acknowledge its end"));
                        }
                    }
                    let taken = ready.take(receipt).await.map(|()| Reply::Done);
                    if receipt {
                        writer.send(&json(&NodeFrame::Reply { id, reply: taken })).await?;
                    } else if let Err(error) = taken {
                        return Err(std::io::Error::other(error.message));
                    }
                }
            }
            continue;
        }
        if let Call::FindFiles { machine, request: fastctx_types::file_search::FileSearchRequest::Open { scope } } =
            call
        {
            let checked = active_caller(node, &hello.peer).and_then(|()| {
                for resident in &arrived.residents {
                    node.residents.active(resident)?;
                }
                Ok(())
            });
            if let Err(error) = checked {
                writer.send(&json(&NodeFrame::Reply { id, reply: Err(error) })).await?;
                return writer.drain().await;
            }
            let acting = match caller::of(node, &hello.peer) {
                Ok(caller) => crate::around::acting_of(node, &caller),
                Err(error) => {
                    writer.send(&json(&NodeFrame::Reply { id, reply: Err(error) })).await?;
                    return writer.drain().await;
                }
            };
            let opened = tokio::select! {
                result = crate::file_search::open(node, &acting, node.machine(), machine, scope) => result,
                _ = reader.recv() => return Ok(()),
            };
            match opened {
                Ok((reply, mut lease)) => {
                    writer.send(&json(&NodeFrame::Reply { id, reply: Ok(Reply::FilesFound(reply)) })).await?;
                    writer.drain().await?;
                    tokio::select! { _ = reader.recv() => {}, () = lease.closed() => {} }
                    return Ok(());
                }
                Err(error) => {
                    writer.send(&json(&NodeFrame::Reply { id, reply: Err(error) })).await?;
                    return writer.drain().await;
                }
            }
        }
        if let Call::On { resident, town, request } = call {
            let receipt = request.receipt;
            let process = request.process.clone();
            // Only the World's shell of that resident sends its `on`s: the
            // command speaks as the resident its job belongs to.
            let ended = match &hello.peer {
                Peer::Command { resident: Some(speaker), .. } if *speaker == resident => {
                    // A script another machine sent here goes on for the
                    // account it was sent for.
                    let account = (resident.machine != node.machine())
                        .then(|| crate::on::sent_for(node, request.job.as_ref()))
                        .flatten();
                    let origin = fastctx_types::on::OnOrigin { resident, town, request: None, follower: None, account };
                    match crate::on::send(node, origin, request).await {
                        Err(failure) => Err(failure),
                        Ok(mut following) => loop {
                            tokio::select! {
                                piece = following.pieces.recv() => match piece {
                                    Some(crate::on::Piece::Value(Ok(piece))) => {
                                        let written = writer.send(&json(&NodeFrame::On { id, piece })).await;
                                        written.or_else(|error| on_unwritten(&following.stop, process.as_ref(), error))?;
                                    }
                                    Some(crate::on::Piece::Value(Err(failure))) => break Err(failure),
                                    Some(crate::on::Piece::End { end, taken }) => {
                                        let piece = fastctx_types::on::OnPiece::Ended { end };
                                        let written = match writer.send(&json(&NodeFrame::On { id, piece })).await {
                                            Ok(()) => writer.drain().await,
                                            Err(error) => Err(error),
                                        };
                                        written.or_else(|error| on_unwritten(&following.stop, process.as_ref(), error))?;
                                        if receipt {
                                            let frame = match reader.recv().await {
                                                Ok(Some(frame)) => frame,
                                                left => {
                                                    on_departed(&following.stop, process.as_ref(), &left)?;
                                                    return Ok(());
                                                }
                                            };
                                            let word: String = parse(&frame)?;
                                            if word == fastctx_types::on::STOP {
                                                let _ = following.stop.send(true);
                                                return Ok(());
                                            }
                                            if word != fastctx_types::on::TAKEN {
                                                return Err(std::io::Error::other("the on client did not acknowledge its end"));
                                            }
                                        }
                                        taken.send(()).map_err(|_| std::io::Error::other("the on completion handover stopped before its receipt"))?;
                                    }
                                    None => break Ok(Reply::Done),
                                },
                                left = reader.recv() => {
                                    on_departed(&following.stop, process.as_ref(), &left)?;
                                    return Ok(());
                                },
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
            // Only the user reads files this way, for this computer's logins.
            let acting = fastctx_types::access::Acting::Here;
            return crate::file_read::serve_local(node, acting, machine, request, id, &mut reader, &mut writer).await;
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
        let cancellable = matches!(&call, Call::Offers { .. });
        let run_resident = match &call { Call::Run { resident, .. } => Some(resident.clone()), _ => None };
        let answer = async {
            match call {
                Call::Collect { resident, answering } => {
                    collect(node, resident, answering, &mut arrived.collected, &mut arrived.notes).await
                }
                Call::Taken { resident, ids, shown, completed } => {
                    let pending = arrived.completed.take().filter(|(owner, _)| *owner == resident).map(|(_, job)| job);
                    let completed = completed.or(pending);
                    taken(node, resident, ids, shown, completed, &mut arrived.collected, &mut arrived.notes).await
                }
                call => handle(node, &hello.peer, call).await,
            }
        };
        let mut reply = if cancellable {
            tokio::select! { reply = answer => reply, _ = reader.recv() => return Ok(()) }
        } else {
            answer.await
        };
        if let Ok(Reply::Run(run)) = &reply
            && !matches!(run.state, fastctx_types::run::RunState::Running | fastctx_types::run::RunState::Exited { .. })
            && let Some(owner) = run_resident
        {
            arrived.completed = Some((owner, fastctx_types::run::CompletedJob {
                machine: run.machine.clone(), job: run.job.clone(), output: run.output.log.clone(),
            }));
        }
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

/// The `on` client said something more, or its connection ended: a stop is
/// on purpose when it says so, or when the client is gone.
fn on_departed(
    stop: &tokio::sync::watch::Sender<bool>,
    process: Option<&fastctx_types::ProcessIdentity>,
    frame: &std::io::Result<Option<Vec<u8>>>,
) -> std::io::Result<()> {
    let deliberate = match frame {
        Ok(Some(frame)) => parse::<String>(frame)? == fastctx_types::on::STOP,
        _ => on_gone(process)?,
    };
    if deliberate {
        let _ = stop.send(true);
    }
    Ok(())
}

/// Writing to the `on` client failed: because it is gone, which is a stop
/// on purpose, or because this end broke, after which the job's end is
/// still owed to the resident. Returns the write's error either way.
fn on_unwritten(
    stop: &tokio::sync::watch::Sender<bool>,
    process: Option<&fastctx_types::ProcessIdentity>,
    error: std::io::Error,
) -> std::io::Result<()> {
    if on_gone(process)? {
        let _ = stop.send(true);
    }
    Err(error)
}

/// Whether the `on` client is gone. A client leaves without a word only
/// when it is stopped with the shell it ran in, and it is gone or going by
/// the time its connection closes (`fastctx_platform::has_ended`). One that
/// sent no identity is taken as gone.
fn on_gone(process: Option<&fastctx_types::ProcessIdentity>) -> std::io::Result<bool> {
    Ok(process.map(fastctx_platform::has_ended).transpose()?.unwrap_or(true))
}

struct Arrived<'a> {
    node: &'a Arc<Node>,
    residents: Vec<ResidentId>,
    collected: Vec<(ResidentId, NoticeId, Option<String>)>,
    notes: Vec<(ResidentId, crate::residents::Heard)>,
    completed: Option<(ResidentId, fastctx_types::run::CompletedJob)>,
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
    let gate = node.residents.management(&resident)?;
    let _collecting = gate.read().await;
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
                                let codex = node.codex.of(&about).ok()?;
                                about.session.and_then(|thread| codex.running_turn(&thread))
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
        if let Some(warning) = node.lifetime.warning() {
            notes.push(format!("=== {warning} ==="));
        }
        // Those it left running where it works, too.
        let elsewhere = crate::here::running_elsewhere(&node, &id)
            .into_iter()
            .filter(|job| Some(&job.job) != answering.as_ref())
            .map(|job| (format!("{} on {}", job.job, job.machine), job.started_at, job.command))
            .collect();
        notes.extend(crate::jobs::still_running(&node.world, &node.machine(), &id, answering.as_ref(), elsewhere)?);
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
    shown: Option<fastctx_types::JobId>,
    completed: Option<fastctx_types::run::CompletedJob>,
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
                crate::speech::settle(node, &directory, &input.delivery.id).await?;
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
    if let Some(completed) = completed {
        let owner = fastctx_types::Address { machine: node.machine(), resident: resident.clone() };
        crate::retention::confirmed(node, owner, completed.machine, fastctx_types::run::JobDelivery {
            shown_whole: shown.as_ref() == Some(&completed.job), job: completed.job,
            output: completed.output, at: jiff::Timestamp::now(),
        }, None).map_err(|error| Failure::new(format!("recording job delivery: {error}")))?;
    } else if let Some(job) = shown
        && !crate::here::forget(node, &resident, &job)
    {
        let (node, owner) = (node.clone(), fastctx_types::Address { machine: node.machine(), resident });
        tokio::spawn(async move { crate::jobs::forget(&node, town, owner, job).await });
    }
    Ok(Reply::Done)
}

fn active_caller(node: &Node, peer: &Peer) -> Result<(), Failure> {
    if let Ok(Caller::Resident(address)) = caller::of(node, peer)
        && address.machine == node.machine()
    {
        node.residents.active(&address.resident)?;
    }
    Ok(())
}

pub(crate) async fn handle(node: &Arc<Node>, peer: &Peer, call: Call) -> Result<Reply, Failure> {
    if node.root.join("uninstalled.json").exists() && !matches!(call, Call::UninstallPrepare | Call::HandOver) {
        return Err(Failure::new("this World is being uninstalled"));
    }
    active_caller(node, peer)?;
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
            let acting = crate::around::acting_of(node, &caller);
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
            let mut reply = who(node, &acting, all, town, since).await?;
            reply.you = you;
            Ok(Reply::Who(reply))
        }
        Call::Since { since, town } => {
            let acting = crate::around::acting_of(node, &caller::of(node, peer)?);
            crate::since::query(node, &acting, since, town).await.map(Reply::Since)
        }
        Call::Look { ids } => {
            let acting = crate::around::acting_of(node, &caller::of(node, peer)?);
            Ok(Reply::Looked { looked: crate::look::look(node, &acting, ids).await })
        }
        Call::StopJob { job, machine, hub } => {
            let caller = caller::of(node, peer)?;
            let acting = crate::around::acting_of(node, &caller);
            let by = match caller {
                Caller::Resident(address) => address.tag(),
                Caller::User => fastctx_types::residents::STOPPED_BY_USER.to_string(),
                Caller::Session(agent) => format!("a {agent} session"),
                Caller::Page(page) => format!("the page {}", page.page),
            };
            let machine = machine.map(|name| crate::reach::Written { name, hub });
            crate::stop::stop(node, &acting, by, job, machine).await
        }
        Call::Talk { to, machine, text, attachments, files_on } => {
            users_voice(node, peer)?;
            crate::talk::talk(node, to, machine, text, attachments, files_on)
                .await
                .map(|(to, landed)| Reply::Spoken { to, landed })
        }
        Call::Agents { machine } => {
            let acting = crate::around::acting_of(node, &caller::of(node, peer)?);
            crate::agents::query(node, &acting, machine).await.map(Reply::Agents)
        }
        Call::ClaudeConsent { machine, accept, home } => {
            users_voice(node, peer)?;
            crate::consent::call(node, machine.clone(), accept, home)
                .await
                .map(|(accepted, changed)| Reply::ClaudeConsent { machine, accepted, changed })
        }
        Call::Homes { machine, change } => {
            users_voice(node, peer)?;
            crate::homes::call(node, machine, change).await.map(|()| Reply::Done)
        }
        Call::FindFiles { machine, request } => {
            let acting = crate::around::acting_of(node, &caller::of(node, peer)?);
            crate::file_search::call(node, &acting, machine, request).await.map(Reply::FilesFound)
        }
        Call::Open { agent, town, node: target, text, model, reasoning_effort, attachments, choices } => {
            users_voice(node, peer)?;
            let options = fastctx_types::agents::SessionOptions { model, reasoning_effort };
            crate::talk::open(node, agent, town, target, text, options, attachments, choices)
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
        Call::Ask { resident, label, prompt, callee, login } => {
            let town = town_of(node, &resident)?;
            crate::colleagues::ask(node, resident, town, label, prompt, callee, login.as_ref())
                .await
                .map(|colleague| Reply::Asked { colleague })
        }
        Call::Message { resident, to, machine, text, login } => {
            let town = town_of(node, &resident)?;
            crate::colleagues::message(node, resident, town, to, machine, text, login.as_ref())
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
        // Logins and accounts are the user's alone: a resident that could
        // log this computer in, grant a machine or make a token could hand
        // the user's machines to anyone.
        Call::Login { hub, account, password, register, name } => {
            users_voice(node, peer)?;
            crate::world::login(node, hub, account, password, register, name).await
        }
        Call::LoginWithToken { token, name } => {
            users_voice(node, peer)?;
            crate::world::login_with_token(node, &token, name).await
        }
        Call::OpenAccount { hub, account, password, register, name } => {
            users_voice(node, peer)?;
            crate::world::open_account(node, hub, account, password, register, name).await
        }
        Call::Account { hub, account, change } => {
            users_voice(node, peer)?;
            crate::world::change(node, hub.as_ref(), &account, change).await.map(|token| Reply::Changed { token })
        }
        Call::Accounts => Ok(Reply::Accounts(crate::world::accounts(node))),
        Call::ServerCommand { hub, account, name, expires } => {
            users_voice(node, peer)?;
            crate::server_invitation::command(node, hub.as_ref(), &account, name, expires)
                .await
                .map(|command| Reply::ServerInvitation { command })
        }
        Call::HandOver => {
            users_voice(node, peer)?;
            crate::handoff::hand_over(node.clone());
            Ok(Reply::Done)
        }
        Call::UninstallPrepare => {
            users_voice(node, peer)?;
            crate::uninstall::prepare(node).await?;
            Ok(Reply::Done)
        }
        Call::Copy { from, to } => {
            let acting = crate::around::acting_of(node, &caller::of(node, peer)?);
            crate::copy::copy(node, &acting, from, to).await.map(Reply::Copied)
        }
        Call::CollabRead { hub, account } => {
            users_voice(node, peer)?;
            crate::world::hub_link(node, Some(&hub))?.collab_read(&account).map(Reply::CollabRead).map_err(Failure::new)
        }
        Call::CollabWrite { hub, account, collab } => {
            users_voice(node, peer)?;
            crate::world::hub_link(node, Some(&hub))?
                .collab_write(&account, collab)
                .await
                .map(|version| Reply::CollabWritten { version })
                .map_err(Failure::new)
        }
        Call::Nodes => nodes(node).await.map(Reply::Nodes),
        Call::Machines { resident, login, thread } => {
            let resident =
                resident.or_else(|| thread.as_deref().and_then(|thread| node.residents.resident_of_thread(thread)));
            machines(node, resident.as_ref(), login.as_ref())
        }
        Call::WorksOn { resident, machine, works_on } => {
            users_voice(node, peer)?;
            crate::here::set_anywhere(node, resident, machine, works_on).await.map(|()| Reply::Done)
        }
        Call::WatchTools { .. } => Err(Failure::new("a tools watch is answered as it goes, not at once")),
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
    // A resident that works on another machine starts its scripts there,
    // and a script of one `on` goes where it names (`world-run.md`, 这里).
    let reply = match crate::here::start(node, &resident, request) {
        (Some(works_on), request) => crate::here::run(node, &resident, town.clone(), request, &works_on).await?,
        (None, request) => {
            // An agent lent to another account may not work where it runs,
            // so nothing starts here, and the machine goes unnamed
            // (`world-network.md`, 共享机器与借出智能体).
            if let Some((about, _)) = node.residents.about(&resident)
                && node.works_here(&about.acting).is_err()
            {
                return Err(Failure::not_run(lent_refusal(node, &script)));
            }
            let owner = Address { machine: here.clone(), resident: resident.clone() };
            node.jobs.run(node, owner.into(), town.clone(), request, None).await?.0
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

/// Why a script of an agent lent to another account did not run: it named
/// the computer the agent runs on, which it may not work on, or it was not
/// a single `on` (`world-run.md`, 这里).
fn lent_refusal(node: &Node, script: &str) -> String {
    match crate::here::single_on(script) {
        Some((machine, _)) if node.is_here(&machine) => fastctx_hands::lent::here_refused(&machine.to_string()),
        _ => fastctx_hands::lent::SCRIPT_REFUSED.to_string(),
    }
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
    node: &Arc<Node>,
    all: bool,
    town: Option<TownName>,
    since: Option<jiff::Timestamp>,
) -> Result<WhoReply, Failure> {
    let reading = node.clone();
    let (world, machine) = (node.world.clone(), node.machine());
    let place = town.clone();
    let mut reply = blocking("looking around", move || {
        crate::who::who(&world, &machine, reading.residents.everyone(), reading.residents.former(), all, place.as_ref())
    })
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
    acting: &fastctx_types::access::Acting,
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
    let reachable: Vec<_> = crate::around::reachable(node, acting, fastctx_types::access::Purpose::Look)
        .into_iter()
        .filter(|machine| asks_for(&machine.name))
        .collect();
    let heard = crate::around::ask_each(reachable, |_| RemoteCall::Who { all, town: town.clone(), since }).await;
    for (machine, heard) in heard {
        machines.push(match heard {
            crate::around::Heard::Answered(RemoteReply::Who(reply)) if reply.machine == machine => {
                MachineWho::Answered { reply }
            }
            crate::around::Heard::Answered(_) => {
                MachineWho::NoAnswer { machine, reason: "it answered something else".to_string() }
            }
            crate::around::Heard::Offline(since) => MachineWho::Offline { machine, since },
            crate::around::Heard::NoAnswer(reason) => MachineWho::NoAnswer { machine, reason },
        });
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
    let mut reply = WorldWho { machine: here, scope, machines, you: None, history: None, others: Vec::new() };
    crate::who::mark_others(node, acting, &mut reply);
    if let Some(since) = since {
        reply.collect_history(since, town.clone());
        crate::since::presence_for_who(node, &mut reply).await?;
    }
    Ok(reply)
}

/// `fastctx nodes`: this machine's link, and each member of the World its
/// scope covers with what it says of itself and where it stands.
/// For a session here: where its shell starts and the other machines it may
/// work on, from the hubs' views as they stand, asking none of them; and
/// where its colleagues live, when it may ask for them.
fn machines(
    node: &Arc<Node>,
    resident: Option<&ResidentId>,
    login: Option<&fastctx_types::collab::Caller>,
) -> Result<Reply, Failure> {
    let placed = resident.and_then(|resident| crate::here::placed(node, resident));
    let acting = placed.as_ref().map(|(acting, _)| acting.clone()).unwrap_or_default();
    let (here, mut others) = crate::world::machines(node, &acting);
    // The session's shell is where it works: that machine is "this one",
    // and the one it lives on is among the others (`world-run.md`, 这里).
    // An agent lent to another account has no shell anywhere, and the
    // machine it runs on is not one it may work on, so it goes unnamed.
    let works_on =
        placed.as_ref().and_then(|(_, works_on)| works_on.clone()).filter(|works_on| !node.is_here(works_on));
    let lent = works_on.is_none() && node.works_here(&acting).is_err();
    let (here, program) = match works_on {
        Some(works_on) => {
            let written = works_on.to_string();
            others.retain(|other| *other != written);
            others.push(here.to_string());
            others.sort();
            (Some(written), Some(here.to_string()))
        }
        None if lent => (None, None),
        None => (Some(here.to_string()), None),
    };
    let collab = crate::collab::choices(node, resident, login)?.map(|choices| choices.offered);
    Ok(Reply::Machines { here, others, program, collab })
}

/// `fastctx nodes`: this computer, and hub by hub the machines its logins
/// may use, each asked what it is like when it is online.
async fn nodes(node: &Arc<Node>) -> Result<NodesReply, Failure> {
    let here = node.machine();
    let links = node.links();
    let mut hubs = Vec::new();
    for link in links {
        let refreshed = link.refresh().await;
        let view = link.view();
        let logins: Vec<fastctx_types::AccountName> =
            view.me().map(|me| me.logins.iter().cloned().collect()).unwrap_or_default();
        let mut asks = Vec::new();
        for machine in view.machines().filter(|machine| machine.name != here) {
            let rights: Vec<_> =
                logins.iter().map(|account| (account.clone(), view.rights(account, &machine.name))).collect();
            let Some(account) = rights.iter().find(|(_, rights)| rights.hands).map(|(account, _)| account.clone())
            else {
                continue;
            };
            let online = refreshed.is_ok().then_some(machine.online);
            let written = node.written(&link, &machine.name).to_string();
            let unknown = refreshed.as_ref().err().cloned();
            let (link, name, since, through, owner) =
                (link.clone(), machine.name.clone(), machine.since, machine.through.clone(), machine.home.clone());
            asks.push(tokio::spawn(async move {
                let facts = match online {
                    Some(true) => {
                        match link.call(&name, &account, fastctx_types::access::Purpose::Look, RemoteCall::Facts).await
                        {
                            Ok(Ok(RemoteReply::Facts(facts))) => Ok(facts),
                            Ok(Ok(_)) => Err("it answered something else".to_string()),
                            Ok(Err(failure)) => Err(failure.message),
                            Err(failure) => Err(failure.to_string()),
                        }
                    }
                    Some(false) => Err("it is offline".to_string()),
                    None => Err(format!("the hub could not be asked: {}", unknown.unwrap_or_default())),
                };
                NodeView { name, written, online, since, through, facts, owner, rights }
            }));
        }
        let mut machines = Vec::new();
        for ask in asks {
            machines.push(ask.await.map_err(|error| Failure::new(format!("asking around: {error}")))?);
        }
        hubs.push(fastctx_types::world::HubNodes {
            hub: link.membership().id.clone(),
            name: view.hub_name.clone(),
            link: link.state(),
            logins,
            home: view.me().and_then(|me| me.home.clone()),
            machines,
        });
    }
    Ok(NodesReply { machine: here, facts: Some(crate::world::facts(node)), hubs })
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
