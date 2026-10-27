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
        goal_runtime: node.goals.available(),
        sentinels: true,
        goal_commands: true,
        profiles: true,
        account_records: true,
        conversation_plans: true,
        scratch_locations: true,
        machine: node.machine(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        node_lifetime: Some(Box::new(node.lifetime.clone())),
    };
    writer.send(&json(&welcome)).await?;
    // The residents that arrived on this connection are here until it
    // closes, however it closes; what was collected for them and not taken
    // is free to be handed over again then.
    let mut arrived =
        Arrived { node, residents: Vec::new(), collected: Vec::new(), notes: Vec::new(), completed: None };
    let caller = calling(&hello.peer);
    while let Some(frame) = reader.recv().await? {
        let requested = tokio::time::Instant::now();
        let _doing = fastctx_platform::doing::doing(format!("{} from {caller}", crate::labels::call_in(&frame)));
        let ClientFrame::Call { id, call } = (match parse(&frame) {
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
        }) else {
            return Err(std::io::Error::other("an MCP frame requires its own relay connection"));
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
        // A conversation of this machine is reached only by whom it works
        // for, as from another machine (`admission::local`).
        if let Err(failure) = crate::admission::local::admit(node, &hello.peer, &call) {
            writer.send(&json(&NodeFrame::Reply { id, reply: Err(failure) })).await?;
            writer.drain().await?;
            continue;
        }
        if let Call::McpRelay { name } = call {
            return crate::mcp::local::serve(node, &hello.peer, name, id, reader, writer).await;
        }
        if let Call::Watch { resident, machine, composer, held, now, goal } = call {
            // Watching a conversation anywhere is the user's, with this
            // computer's logins.
            if let Err(failure) = users_voice(node, &hello.peer) {
                writer.send(&json(&NodeFrame::Reply { id, reply: Err(failure) })).await?;
                continue;
            }
            // A watch keeps the connection until one end leaves.
            let opening = tokio::select! {
                result = crate::talk::watch(node, resident, machine, composer, held, now, goal) => result,
                _ = reader.recv() => return Ok(()),
            };
            let ended = match opening {
                Err(failure) => Err(failure),
                Ok(mut watched) => loop {
                    tokio::select! {
                        piece = watched.recv() => match piece {
                            Some(Ok(piece)) => writer.send(&json(&NodeFrame::Watched { id, watched: piece })).await?,
                            Some(Err(failure)) => break Err(failure),
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
        if let Call::HoldHost { resident } = call {
            let lease = match users_voice(node, &hello.peer) {
                Ok(()) => {
                    crate::gate_waits::asked_since(requested, crate::lifecycle::attached::hold(node, &resident)).await
                }
                Err(error) => Err(error),
            };
            match lease {
                Ok(_lease) => {
                    writer.send(&json(&NodeFrame::Reply { id, reply: Ok(Reply::Done) })).await?;
                    writer.drain().await?;
                    let _ = reader.recv().await;
                    node.log.line(format_args!("lifecycle {resident}: official interface detached"));
                    return Ok(());
                }
                Err(error) => {
                    writer.send(&json(&NodeFrame::Reply { id, reply: Err(error) })).await?;
                    continue;
                }
            }
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
        if let Call::WatchAccountRecords { hub, account, kind } = call {
            return crate::account_records::watch(node, &hello.peer, hub, account, kind, id, reader, writer).await;
        }
        if let Call::WatchMemory { town } = &call {
            if let Err(failure) = users_voice(node, &hello.peer) {
                writer.send(&json(&NodeFrame::Reply { id, reply: Err(failure) })).await?;
                return writer.drain().await;
            }
            let mut changes = node.memory_changes.subscribe();
            let mut views = node.views.subscribe();
            let mut told = None;
            let ready = crate::town::access::ready(node, &fastctx_types::access::Acting::Here, town).await;
            if let Err(failure) = ready {
                writer.send(&json(&NodeFrame::Reply { id, reply: Err(failure) })).await?;
                return writer.drain().await;
            }
            loop {
                changes.borrow_and_update();
                views.borrow_and_update();
                match crate::memory::api::catalog(node, &Caller::User, town).await {
                    Ok(catalog) if told.as_ref() != Some(&catalog) => {
                        let reply =
                            Reply::Memory(fastctx_types::memory::MemoryReply::Catalog { catalog: catalog.clone() });
                        writer.send(&json(&NodeFrame::Reply { id, reply: Ok(reply) })).await?;
                        told = Some(catalog);
                    }
                    Ok(_) => {}
                    Err(failure) => {
                        writer.send(&json(&NodeFrame::Reply { id, reply: Err(failure) })).await?;
                        return writer.drain().await;
                    }
                }
                tokio::select! {
                    changed = changes.changed() => if changed.is_err() { return Ok(()) },
                    changed = views.changed() => if changed.is_err() { return Ok(()) },
                    _ = reader.recv() => return Ok(()),
                }
            }
        }
        if let Call::WatchFeatures = call {
            // An interface follows the switches for as long as it is open:
            // as they are first, then after each change.
            let mut changes = node.features.subscribe();
            let mut told = None;
            loop {
                match crate::features::read(&node.world) {
                    Ok(features) if told.as_ref() != Some(&features) => {
                        writer.send(&json(&NodeFrame::Features { id, features: features.clone() })).await?;
                        told = Some(features);
                    }
                    Ok(_) => {}
                    Err(failure) => {
                        writer.send(&json(&NodeFrame::Reply { id, reply: Err(failure) })).await?;
                        return writer.drain().await;
                    }
                }
                tokio::select! {
                    changed = changes.changed() => if changed.is_err() { return Ok(()) },
                    _ = reader.recv() => return Ok(()),
                }
            }
        }
        if let Call::WatchWorld = call {
            if let Err(failure) = users_voice(node, &hello.peer) {
                writer.send(&json(&NodeFrame::Reply { id, reply: Err(failure) })).await?;
                return writer.drain().await;
            }
            // An interface follows the World for as long as it is open: as
            // it is first, then after each change (`world-network.md`, 近况).
            let _open = crate::updates::Open::new();
            let mut residents = node.residents.changes();
            let mut shown = node.shown.subscribe();
            let mut jobs = node.jobs.subscribe();
            let mut told = None;
            // What could not be read is read again in a moment, the watch
            // kept: the interface goes on showing what it was told, and the
            // log says why, once for each reason.
            let mut failing: Option<String> = None;
            loop {
                let mut again = false;
                if crate::shown::ready(node) {
                    match crate::shown::now(node).await {
                        Ok(world) => {
                            failing = None;
                            let frame = json(&NodeFrame::World { id, world: Box::new(world) });
                            if told.as_ref() != Some(&frame) {
                                writer.send(&frame).await?;
                                told = Some(frame);
                            }
                        }
                        Err(failure) => {
                            if failing.as_deref() != Some(failure.message.as_str()) {
                                node.log.line(format_args!(
                                    "reading the World for an interface: {}; reading it again in a moment",
                                    failure.message
                                ));
                                failing = Some(failure.message);
                            }
                            again = true;
                        }
                    }
                }
                tokio::select! {
                    () = tokio::time::sleep(std::time::Duration::from_secs(1)), if again => {}
                    changed = residents.changed() => if changed.is_err() { return Ok(()) },
                    changed = shown.changed() => if changed.is_err() { return Ok(()) },
                    job = jobs.recv() => {
                        if matches!(job, Err(tokio::sync::broadcast::error::RecvError::Closed)) {
                            return Ok(());
                        }
                    }
                    _ = reader.recv() => return Ok(()),
                }
            }
        }
        if let Call::Reopened { machine } = call {
            // An interface pausing before it tries again waits here for a
            // way to open; its leaving ends the wait.
            let reply = tokio::select! {
                reply = reopened(node, machine) => reply,
                _ = reader.recv() => return Ok(()),
            };
            writer.send(&json(&NodeFrame::Reply { id, reply })).await?;
            continue;
        }
        if let Call::Wait { jobs, receipt, parent } = call {
            // A wait keeps the connection until the jobs end; its leaving
            // hands back the ends the wait held (`waiting`).
            let (caller, acting, here) = match caller::of(node, &hello.peer) {
                Ok(caller) => {
                    let acting = crate::around::acting_of(node, &caller);
                    let here = crate::around::works_here(node, &acting);
                    (if let Caller::Resident(resident) = caller { Some(resident) } else { None }, acting, here)
                }
                Err(error) => {
                    writer.send(&json(&NodeFrame::Reply { id, reply: Err(error) })).await?;
                    continue;
                }
            };
            // The jobs waited for here are this machine's: only whoever works
            // on it waits for them, as only they start them here.
            let allowed = if here { Ok(()) } else { node.works_here(&acting).map_err(Failure::new) };
            if let Err(error) = allowed.and_then(|()| crate::managed::refuse(node)) {
                writer.send(&json(&NodeFrame::Reply { id, reply: Err(error) })).await?;
                continue;
            }
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
            let opening = crate::file_search::open(node, &acting, node.machine(), machine, scope);
            tokio::pin!(opening);
            let opened = tokio::select! {
                result = &mut opening => result,
                _ = reader.recv() => {
                    drop(opening.await);
                    return Ok(());
                },
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
                    let account = (!node.is_me(&resident.machine))
                        .then(|| crate::on::sent_for(node, request.job.as_ref()))
                        .flatten();
                    let origin = fastctx_types::on::OnOrigin {
                        turn: None,
                        sentinel: None,
                        call_id: None,
                        resident,
                        town,
                        request: None,
                        follower: None,
                        account,
                    };
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
        let Some(answering) = node.handoff.call() else {
            let reply = Err(Failure::new(format!(
                "the node on {} is handing over to its successor; ask again in a moment",
                node.machine()
            )));
            writer.send(&json(&NodeFrame::Reply { id, reply })).await?;
            continue;
        };
        // A copy holds the node back only while a try of it runs (`copy`).
        let _answering = (!matches!(call, Call::Copy { .. })).then_some(answering);
        if let Call::ReadFile { machine, request } = call {
            if let Err(failure) = users_voice(node, &hello.peer) {
                writer.send(&json(&NodeFrame::Reply { id, reply: Err(failure) })).await?;
                return writer.drain().await;
            }
            // Only the user reads files this way, for this computer's logins.
            let acting = fastctx_types::access::Acting::Here;
            return crate::file_read::serve_local(node, acting, machine, request, id, &mut reader, &mut writer).await;
        }
        if let Call::ReadAttachment { attachment, range } = call {
            if let Err(failure) = users_voice(node, &hello.peer) {
                writer.send(&json(&NodeFrame::Reply { id, reply: Err(failure) })).await?;
                return writer.drain().await;
            }
            return crate::file_read::serve_attachment_local(node, attachment, range, id, &mut reader, &mut writer)
                .await;
        }
        let only_notes = matches!(call, Call::ReadLineNotes { .. });
        if let Call::ReadLines { machine, request } | Call::ReadLineNotes { machine, request } = call {
            if let Err(error) = users_voice(node, &hello.peer) {
                writer.send(&json(&NodeFrame::Reply { id, reply: Err(error) })).await?;
                return writer.drain().await;
            }
            return crate::lines::reading::local(node, machine, request, only_notes, id, &mut reader, &mut writer)
                .await;
        }
        if let Call::Lines { machine, request: fastctx_types::lines::Request::Open { path, decoding } } = call {
            if let Err(error) = users_voice(node, &hello.peer) {
                writer.send(&json(&NodeFrame::Reply { id, reply: Err(error) })).await?;
                return writer.drain().await;
            }
            let opening = crate::lines::open(node, node.machine(), machine, path, decoding);
            tokio::pin!(opening);
            let opened = tokio::select! {
                result = &mut opening => result,
                _ = reader.recv() => {
                    drop(opening.await);
                    return Ok(());
                },
            };
            match opened {
                Ok((reply, mut lease)) => {
                    writer.send(&json(&NodeFrame::Reply { id, reply: Ok(Reply::Lines(reply)) })).await?;
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
        if let Call::Upload { target, node: machine, file, composer } = call {
            let reply = match users_voice(node, &hello.peer) {
                Ok(()) => crate::gate_waits::asked_since(
                    requested,
                    crate::attachments::upload(node, target, machine, file, composer, reader),
                )
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
        // What only reads or waits stops once its client is gone; a change
        // is done whole (`Call::on_leaving`). A copy goes on through every
        // break while its caller waits for it, and stops once it is gone.
        let cancellable = call.on_leaving() == fastctx_types::contract::Leaving::Stop;
        let run_resident = match &call {
            Call::Run { resident, .. } => Some(resident.clone()),
            _ => None,
        };
        if run_resident.is_some() {
            arrived.completed = None;
        }
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
                call => {
                    let internal = matches!(
                        call,
                        Call::Arrive { .. }
                            | Call::ClaudeSessionDirectory { .. }
                            | Call::JobChanged { .. }
                            | Call::Stepped { .. }
                            | Call::Saw { .. }
                            | Call::TurnEvidence { .. }
                            | Call::StoreRunFacts { .. }
                            | Call::RecordRunFailure { .. }
                    );
                    if internal {
                        handle(node, &hello.peer, call).await
                    } else {
                        crate::gate_waits::asked_since(requested, Box::pin(handle(node, &hello.peer, call))).await
                    }
                }
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
            arrived.completed = Some((
                owner,
                fastctx_types::run::CompletedJob {
                    machine: run.machine.clone(),
                    job: run.job.clone(),
                    output: run.output.log.clone(),
                },
            ));
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
        for (resident, heard) in &self.notes {
            crate::context::release(self.node, resident, heard);
            crate::scratch::release(self.node, resident, heard);
        }
        for resident in self.node.residents.left(&self.residents) {
            crate::codex::native_context::forget(self.node, &resident);
            crate::colleagues::left(self.node, resident);
        }
        for resident in &self.residents {
            crate::codex::native::left(self.node, resident);
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
    let _collecting = gate.write().await;
    // What the host did to its context, and the blocks kept for it, are read
    // from elsewhere; when that fails, the blocks wait for a later answer, and
    // what waits for the conversation still goes with this one.
    if let Err(failure) = crate::codex::native_context::observe(node, &resident).await {
        node.log.line(format_args!("{resident}: reading its host's context: {}", failure.message));
    }
    crate::context::plugin_heard(node, &resident).map_err(|error| Failure::new(error.to_string()))?;
    if let Err(failure) = crate::context::refresh(node, &resident).await {
        node.log.line(format_args!("{resident}: refreshing its context: {}", failure.message));
    }
    if native_delivery(node, &resident) {
        return collect_notes(node, resident, answering, Vec::new(), collected_notes).await;
    }
    let speaking = node.residents.speaking(&resident).ok_or_else(|| Failure::new("unknown resident"))?;
    let _speaking = speaking.lock().await;
    if native_delivery(node, &resident) {
        return collect_notes(node, resident, answering, Vec::new(), collected_notes).await;
    }
    present(node, &resident)?;
    let directory = node.world.conversation(&resident);
    let kept = blocking("reading what waits", move || crate::inbox::pending(&directory)).await?;
    let mut notices = Vec::new();
    {
        let mut taking = node.taking.lock().expect("never poisoned");
        for pending in kept.into_iter().filter(|pending| pending.state == PendingState::Waiting) {
            present(node, &resident)?;
            let directory = node.world.conversation(&resident);
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
                            if about.world_managed() && about.host.agent.as_str() == "codex" {
                                let codex = node.codex.of(node, &about).ok()?;
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
        (about.host.agent.as_str() == "codex"
            && (about.world_started
                || about
                    .session
                    .as_ref()
                    .is_some_and(|thread| node.codex.of(node, &about).is_ok_and(|host| host.loaded(thread)))))
            || crate::claude::paired(node, resident)
    })
}

async fn collect_notes(
    node: &Arc<Node>,
    resident: ResidentId,
    answering: Option<fastctx_types::JobId>,
    notices: Vec<fastctx_types::residents::Delivery>,
    collected_notes: &mut Vec<(ResidentId, crate::residents::Heard)>,
) -> Result<Reply, Failure> {
    let (reading, id) = (node.clone(), resident.clone());
    let (mut notes, mut leading_notes, heard) = blocking("reading the town", move || {
        let node = &reading;
        let (mut notes, mut heard) = crate::town::notes::notes(node, &id)?;
        let (paths, scratch) = crate::scratch::collect(node, &id, notes.len())?;
        notes.extend(paths);
        if let Some(heard) = &mut heard {
            heard.scratch = Some(scratch);
        }
        let leading_notes = notes.len();
        if let Some(warning) = node.lifetime.warning() {
            notes.push(format!("=== {warning} ==="));
        }
        // Those it left running where it works, too.
        let elsewhere = crate::here::running_elsewhere(node, &id)
            .into_iter()
            .filter(|job| Some(&job.job) != answering.as_ref())
            .map(|job| (format!("{} on {}", job.job, job.machine), job.started_at, job.command))
            .collect();
        notes.extend(crate::jobs::still_running(&node.world, &node.names(), &id, answering.as_ref(), elsewhere)?);
        Ok((notes, leading_notes, heard))
    })
    .await?;
    if let Some(mut heard) = heard {
        if let Some(omitted) = crate::scratch::reserve(node, &resident, &mut heard) {
            leading_notes -= omitted.len();
            notes.drain(omitted);
        }
        if let Some(omitted) = crate::context::reserve(node, &resident, &mut heard) {
            leading_notes -= omitted.len();
            notes.drain(omitted);
        }
        collected_notes.push((resident, heard));
    }
    Ok(Reply::Notices { notices, notes, leading_notes })
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
    // Where a job an older program ran for it was put: in its project then.
    let town = town_of(node, &resident)?;
    let directory = node.world.conversation(&resident);
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
                pending.readers = if crate::goals::replies::inherits(node, &resident)
                    .map_err(|error| Failure::new(error.to_string()))?
                {
                    node.residents.working_for(&resident)
                } else {
                    Vec::new()
                };
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
    for (owner, heard) in collected_notes.iter_mut().filter(|(owner, _)| *owner == resident) {
        crate::context::acknowledge(node, owner, heard).map_err(|error| Failure::new(error.to_string()))?;
        crate::scratch::acknowledge(node, owner, heard).map_err(|error| Failure::new(error.to_string()))?;
        node.residents.acknowledge_heard(owner, heard.clone());
    }
    collected_notes.retain(|(owner, _)| *owner != resident);
    if let Some(completed) = completed {
        let owner = fastctx_types::Address { machine: node.machine(), resident: resident.clone() };
        crate::retention::confirmed(
            node,
            owner,
            completed.machine,
            fastctx_types::run::JobDelivery {
                shown_whole: shown.as_ref() == Some(&completed.job),
                job: completed.job,
                output: completed.output,
                at: jiff::Timestamp::now(),
            },
            None,
        )
        .map_err(|error| Failure::new(format!("recording job delivery: {error}")))?;
    } else if let Some(job) = shown
        && !crate::here::forget(node, &resident, &job)
    {
        let (node, owner) = (node.clone(), fastctx_types::Address { machine: node.machine(), resident });
        tokio::spawn(async move { crate::jobs::forget(&node, town, owner, job).await });
    }
    Ok(Reply::Done)
}

fn active_caller(node: &Node, peer: &Peer) -> Result<(), Failure> {
    if matches!(peer, Peer::Command { .. }) {
        let caller = caller::of(node, peer)?;
        if let Caller::Resident(address) = caller
            && node.is_me(&address.machine)
        {
            node.residents.active(&address.resident)?;
        }
    }
    Ok(())
}

pub(crate) async fn handle(node: &Arc<Node>, peer: &Peer, call: Call) -> Result<Reply, Failure> {
    if node.root.join("uninstalled.json").exists() && !matches!(call, Call::UninstallPrepare | Call::HandOver) {
        return Err(Failure::not_run("this World is being uninstalled"));
    }
    active_caller(node, peer)?;
    match call {
        Call::Goal { resident, machine, account, request } => {
            users_voice(node, peer)?;
            crate::goals::calls::user(node, resident, machine, account, request)
                .await
                .map(|updated| Reply::Goal { updated: Box::new(updated) })
        }
        Call::GoalCommand { hub, account, job, request } => {
            crate::goals::calls::command(node, peer, hub, account, job, request)
                .await
                .map(|updated| Reply::Goal { updated: Box::new(updated) })
        }
        Call::TurnEvidence { resident, turn, refresh_limits } => {
            users_voice(node, peer)?;
            let evidence = crate::turn_evidence::snapshot(node, &resident, turn, refresh_limits).await?;
            let text = crate::turn_evidence::words::render(
                &evidence,
                &node.world.conversation(&resident).actions().to_string_lossy(),
            );
            Ok(Reply::TurnEvidence { evidence: Box::new(evidence), text })
        }
        Call::CompactBetweenTurns { resident } => {
            users_voice(node, peer)?;
            Ok(Reply::Compacted { result: crate::turn_evidence::compact::between_turns(node, &resident).await })
        }
        Call::Timer { timer } => {
            users_voice(node, peer)?;
            crate::timers::call(node, timer).await
        }
        Call::ClaudeSessionDirectory { session, previous, cwd, process, conversation, home, background } => {
            if !matches!(peer, Peer::Session { host } if host.agent.as_str() == "claude") {
                users_voice(node, peer)?;
            }
            crate::claude::pairing::directory(node, session, previous, cwd, conversation, process, home, background)
                .await
                .map(|path| Reply::ClaudeSessionDirectory { path })
        }
        Call::Arrive { host, hints } => {
            // A managed machine takes no session of its own agents.
            crate::managed::refuse(node)?;
            if host.agent.as_str() == "claude" || hints.tool_use_id.is_some() {
                let id = hints.tool_use_id.as_deref().filter(|id| !id.is_empty()).ok_or_else(|| {
                    Failure::new("Claude did not supply a tool use id; its session cannot be identified")
                })?;
                if let Some(resident) = &hints.resident {
                    node.residents.active(resident)?;
                }
                let whereabouts = crate::claude::identify(node, id).await?;
                if hints.resident.is_some_and(|resident| resident != whereabouts.resident) {
                    return Err(fastctx_types::authorization::Denial::new(
                        fastctx_types::authorization::DenialReason::NoSubject,
                        "the Claude call belongs to a different conversation",
                    )
                    .into());
                }
                return Ok(Reply::Arrived(whereabouts));
            }
            let codex = host.agent.as_str() == "codex";
            let process = hints.host_process.clone();
            // A session the World does not know is the owner's own when the
            // user's own program runs it; one the World's Codex host runs,
            // under a login that assists other accounts, may be a thread one
            // of their conversations spawned (`crate::adoption`).
            let unknown = hints.resident.is_none()
                && hints.was.is_none()
                && hints.thread.as_deref().is_none_or(|thread| node.residents.resident_of_thread(thread).is_none());
            let own = hints
                .host_process
                .as_ref()
                .is_some_and(|started| !node.codex.all().iter().any(|world| world.process().as_ref() == Some(started)));
            let arrival = crate::adoption::Arrival { login: crate::adoption::Login::Named(hints.home.clone()), own };
            if unknown && let Err(error) = crate::adoption::owners_own(node, host.agent.as_str(), &arrival) {
                node.log.line(format_args!("refused a {} session the World did not start: {error}", host.agent));
                return Err(Failure::new(error.to_string()));
            }
            let whereabouts = node.residents.arrive(&node.world, &node.machine(), host, hints)?;
            if let Err(error) = crate::codex::native_context::observe(node, &whereabouts.resident).await {
                node.log.line(format_args!("initializing {}'s context observation: {error}", whereabouts.resident));
            }
            if codex
                && node.residents.about(&whereabouts.resident).is_some_and(|(about, _)| !about.world_started)
                && let Err(error) = crate::codex::native_lifetime::record(&node.world, &whereabouts.resident, process)
            {
                node.log.line(format_args!("recording the native host of {}: {error}", whereabouts.resident));
            }
            Ok(Reply::Arrived(whereabouts))
        }
        Call::Run { resident, request, node: target, call_id } => {
            let town = town_of(node, &resident)?;
            let reply = run(node, resident.clone(), town, request, target, call_id.as_deref()).await?;
            Ok(Reply::Run(reply))
        }
        Call::StoreRunFacts { resident, call_id, facts } => {
            if !matches!(peer, Peer::Session { .. }) {
                return Err(Failure::new("tool call facts come from a session's adapter"));
            }
            crate::run_facts::keep_for(node, &resident, &call_id, &facts)
                .map_err(|error| Failure::new(format!("recording tool call facts: {error}")))?;
            Ok(Reply::Done)
        }
        Call::RecordRunFailure { resident, call_id, error } => {
            if !matches!(peer, Peer::Session { .. }) {
                return Err(Failure::new("tool call facts come from a session's adapter"));
            }
            crate::run_facts::record_failure(node, &resident, &call_id, &error)
                .map_err(|error| Failure::new(format!("recording tool call facts: {error}")))?;
            Ok(Reply::Done)
        }
        Call::Write { resident: Some(resident), op, node: target, .. } => {
            let town = town_of(node, &resident)?;
            write(node, resident, town, op, target).await.map(|outcome| Reply::Written { outcome })
        }
        Call::Write { resident: None, op, node: target, town, job } => {
            if target.is_some() {
                return Err(Failure::new(
                    "write commands run on this machine; run the command on the destination machine",
                ));
            }
            crate::write_command::write(node, peer, town, op, job).await.map(|outcome| Reply::Written { outcome })
        }
        Call::JobChanged { job, owner, town } => node.jobs.changed(node, job, owner, town).await.map(|()| Reply::Done),
        Call::Stepped { job, town, id, began, ended, did } => {
            let told = crate::step_command::Told { job, town, id, began, ended, did };
            crate::step_command::stepped(node, peer, told).await.map(|()| Reply::Done)
        }
        Call::Saw { owner, town: _, seen } => {
            let (world, here) = (node.world.clone(), node.names());
            blocking("recording what was seen", move || crate::residents::record_seen(&world, &here, &owner, &seen))
                .await
                .map(|()| Reply::Done)
        }
        Call::Who { all, town, since } => {
            let caller = caller::of(node, peer)?;
            let town = town.or_else(|| caller.town(node));
            let acting = crate::around::acting_of(node, &caller);
            let here_too = crate::around::works_here(node, &acting);
            let you = match caller {
                caller::Caller::Resident(address) => Some(address),
                _ => None,
            };
            let since = match since {
                Some(at) => Some(at),
                None => match &you {
                    Some(address) if node.is_me(&address.machine) => {
                        let world = node.world.clone();
                        let id = address.resident.clone();
                        blocking("reading the previous turn", move || {
                            crate::activity::last_turn(&world.conversation(&id))
                        })
                        .await?
                    }
                    _ => None,
                },
            };
            let mut reply = who(node, &acting, here_too, all, town, since).await?;
            reply.you = you;
            Ok(Reply::Who(reply))
        }
        Call::Since { since, town } => {
            let caller = caller::of(node, peer)?;
            let acting = crate::around::acting_of(node, &caller);
            let here_too = crate::around::works_here(node, &acting);
            crate::since::query(node, &acting, here_too, since, town).await.map(Reply::Since)
        }
        Call::Look { ids } => {
            let caller = caller::of(node, peer)?;
            let acting = crate::around::acting_of(node, &caller);
            let here_too = crate::around::works_here(node, &acting);
            Ok(Reply::Looked { looked: crate::look::look(node, &acting, here_too, ids).await })
        }
        Call::StopJob { job, machine, hub } => {
            let caller = caller::of(node, peer)?;
            let acting = crate::around::acting_of(node, &caller);
            let here_too = crate::around::works_here(node, &acting);
            let by = match caller {
                Caller::Resident(address) => address.tag(),
                Caller::User => fastctx_types::residents::STOPPED_BY_USER.to_string(),
                Caller::Session(agent) => format!("a {agent} session"),
                Caller::Page(page) => format!("the page {}", page.page),
            };
            let machine = machine.map(|name| crate::reach::Written { name, hub });
            crate::stop::stop(node, &acting, here_too, by, job, machine).await
        }
        Call::Talk { to, machine, text, attachments, files_on, id } => {
            users_voice(node, peer)?;
            // Named as the machine they go to names them: by the machine
            // they were typed on and the device's own id for them.
            let delivery = id.as_deref().map(|id| crate::speech::fixed_id("talk", &[node.machine().as_str(), id]));
            crate::talk::talk(node, to, machine, text, attachments, files_on, id)
                .await
                .map(|(to, landed)| Reply::Spoken { to, landed, delivery })
        }
        Call::Agents { machine } => {
            let acting = crate::around::acting_of(node, &caller::of(node, peer)?);
            crate::agents::query(node, &acting, machine).await.map(Reply::Agents)
        }
        Call::ClaudeConsent { machine, accept, home } => {
            users_voice(node, peer)?;
            crate::managed::refuse(node)?;

            crate::consent::call(node, machine.clone(), accept, home)
                .await
                .map(|(accepted, changed)| Reply::ClaudeConsent { machine: machine.name, accepted, changed })
        }
        Call::Homes { machine, change } => {
            users_voice(node, peer)?;
            crate::homes::call(node, machine, change).await.map(|()| Reply::Done)
        }
        Call::AgentLogins { machine } => {
            let acting = crate::around::acting_of(node, &caller::of(node, peer)?);
            crate::logins::query(node, &acting, machine).await.map(Reply::AgentLogins)
        }
        Call::ScanAgents { machine, stop } => {
            users_voice(node, peer)?;
            crate::managed::refuse(node)?;

            crate::logins::remote::scan(node, machine, stop)
                .await
                .map(|logins| Reply::MachineAgentLogins(Box::new(logins)))
        }
        Call::AddAgentLogin { machine, agent, dir } => {
            users_voice(node, peer)?;
            crate::managed::refuse(node)?;

            crate::logins::remote::add(node, machine, agent, dir)
                .await
                .map(|logins| Reply::MachineAgentLogins(Box::new(logins)))
        }
        Call::AgentFolder { machine, dir, change } => {
            users_voice(node, peer)?;
            crate::managed::refuse(node)?;

            crate::logins::remote::folder(node, machine, dir, change)
                .await
                .map(|logins| Reply::MachineAgentLogins(Box::new(logins)))
        }
        Call::SetAgentTitle { machine, agent, dir, on } => {
            users_voice(node, peer)?;
            crate::managed::refuse(node)?;

            crate::logins::remote::set_title(node, machine, agent, dir, on)
                .await
                .map(|logins| Reply::MachineAgentLogins(Box::new(logins)))
        }
        Call::PreviewAgentChange { machine, agent, dir, into, take_back } => {
            users_voice(node, peer)?;
            crate::managed::refuse(node)?;

            crate::logins::remote::preview(node, machine, agent, dir, into, take_back)
                .await
                .map(|preview| Reply::AgentChangePreview(Box::new(preview)))
        }
        Call::ChangeAgentLogin { machine, agent, dir, into, raise_output_limit, take_back } => {
            users_voice(node, peer)?;
            crate::managed::refuse(node)?;

            crate::logins::remote::change(node, machine, agent, dir, into, raise_output_limit, take_back)
                .await
                .map(|change| Reply::AgentLoginChanged(Box::new(change)))
        }
        Call::ReadAgentLimits { machine, agent, dir } => {
            users_voice(node, peer)?;
            crate::logins::remote::read_limits(node, machine, agent, dir)
                .await
                .map(|logins| Reply::MachineAgentLogins(Box::new(logins)))
        }
        Call::AgentCalls { machine, agent, dir, query } => {
            users_voice(node, peer)?;
            crate::logins::remote::calls(node, machine, agent, dir, query)
                .await
                .map(|page| Reply::AgentCalls(Box::new(page)))
        }
        Call::AgentUsage { machine, agent, dir } => {
            users_voice(node, peer)?;
            crate::logins::remote::usage(node, machine, agent, dir)
                .await
                .map(|report| Reply::AgentUsage(Box::new(report)))
        }
        Call::FindFiles { machine, request } => {
            let acting = crate::around::acting_of(node, &caller::of(node, peer)?);
            crate::file_search::call(node, &acting, machine, request).await.map(Reply::FilesFound)
        }
        Call::Folders { machine, path } => {
            users_voice(node, peer)?;
            let acting = crate::around::acting_of(node, &caller::of(node, peer)?);
            crate::focus::folders(node, &acting, machine, path).await.map(Reply::Folders)
        }
        Call::Stats { machine, after, seen } => {
            users_voice(node, peer)?;
            let acting = crate::around::acting_of(node, &caller::of(node, peer)?);
            crate::stats::query(node, &acting, machine, after, seen).await.map(|stats| Reply::Stats(Box::new(stats)))
        }
        Call::Open { agent, town, node: target, text, model, reasoning_effort, attachments, choices } => {
            users_voice(node, peer)?;
            crate::managed::refuse(node)?;

            let options = fastctx_types::agents::SessionOptions { model, reasoning_effort, ..Default::default() };
            crate::talk::open(node, agent, town, target, text, options, attachments, choices)
                .await
                .map(|(resident, landed)| Reply::Opened { resident, landed })
        }
        Call::Collaboration { resident, machine } => {
            users_voice(node, peer)?;
            crate::colleagues::tree::query(node, resident, machine)
                .await
                .map(|tree| Reply::Collaboration(Box::new(tree)))
        }
        Call::Stop { resident, machine } => {
            if let Caller::Resident(parent) = caller::of(node, peer)? {
                return crate::colleagues::stopping::stop(node, parent, resident, machine).await;
            }
            users_voice(node, peer)?;
            crate::talk::stop(node, resident, machine)
                .await
                .map(|(resident, stopped)| Reply::Stopped { resident, stopped })
        }
        Call::Data(call) => {
            // What this machine keeps is the user's to read and change.
            users_voice(node, peer)?;
            crate::data::answer(node, call).await.map(Reply::Data)
        }
        Call::Update(call) => {
            // Which versions this machine runs is the user's to choose.
            users_voice(node, peer)?;
            crate::updates::call(node, call).await.map(|state| Reply::Update(Box::new(state))).map_err(Failure::new)
        }
        Call::UpdateMachine { machine } => {
            users_voice(node, peer)?;
            let acting = crate::around::acting_of(node, &caller::of(node, peer)?);
            let update = fastctx_types::update::UpdateCall::Kernel { version: None };
            crate::updates::machine(node, &acting, machine, update).await.map(|state| Reply::Update(Box::new(state)))
        }
        Call::MachineUpdates { machine, update } => {
            users_voice(node, peer)?;
            let acting = crate::around::acting_of(node, &caller::of(node, peer)?);
            crate::updates::machine(node, &acting, machine, update).await.map(|state| Reply::Update(Box::new(state)))
        }
        Call::Manage { resident, machine, action } => {
            users_voice(node, peer)?;
            crate::management::manage(node, resident, machine, action)
                .await
                .map(|(resident, managed)| Reply::Managed { resident, managed })
        }
        Call::Forget { resident, machine } => {
            users_voice(node, peer)?;
            let address = fastctx_types::Address { machine: machine.name.clone(), resident: resident.clone() };
            crate::forget::ask(node, resident, machine).await.map(|how| Reply::Forgetting { resident: address, how })
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
        // What an interface shows of a conversation anywhere, read with this
        // computer's logins: the user's alone.
        Call::ItemDetail { resident, machine, turn, item } => {
            users_voice(node, peer)?;
            crate::held_history::item_detail(node, resident, machine, turn, item)
                .await
                .map(|(resident, detail)| Reply::ItemDetail { resident, detail })
        }
        Call::AgentActivity { resident, machine, agent } => {
            users_voice(node, peer)?;
            crate::agent_activity::read(node, resident, machine, agent)
                .await
                .map(|(resident, activity)| Reply::AgentActivity { resident, activity: Box::new(activity) })
        }
        Call::Offers { resident, machine } => {
            users_voice(node, peer)?;
            crate::control::offers(node, resident, machine).await.map(|(resident, mut offers)| {
                offers.control_reply_envelope = false;
                Reply::Offers { resident, offers: Box::new(offers) }
            })
        }
        Call::Attach { resident } => {
            users_voice(node, peer)?;
            crate::attach::command(node, &resident).await.map(Reply::Attachable)
        }
        Call::HoldHost { .. } => Err(Failure::new("a host lease uses its own connection")),
        Call::Watch { .. } => Err(Failure::new("a watch is answered as it goes, not at once")),
        Call::On { .. } => Err(Failure::new("an on is answered as it goes, not at once")),
        Call::ReadFile { .. } => Err(Failure::new("a file read is answered as a byte stream")),
        Call::ReadAttachment { .. } => Err(Failure::new("an attachment read is answered as a byte stream")),
        Call::ReadLines { .. } | Call::ReadLineNotes { .. } => {
            Err(Failure::new("logical rows are answered as a text stream"))
        }
        Call::Thumbnail { machine, request } => {
            users_voice(node, peer)?;
            crate::thumbnail::call(node, machine, request).await.map(Reply::Thumbnail)
        }
        Call::AttachmentThumbnail { attachment, max } => {
            users_voice(node, peer)?;
            crate::thumbnail::attachment(node, attachment, max).await.map(Reply::Thumbnail)
        }
        Call::Lines { machine, request } => {
            users_voice(node, peer)?;
            crate::lines::call(node, node.machine(), machine, request).await.map(Reply::Lines)
        }
        Call::AttachmentPlaces { target, machine, account, preferred } => {
            users_voice(node, peer)?;
            crate::attachments::destinations::query(node, target, machine, account, preferred)
                .await
                .map(Reply::AttachmentPlaces)
        }
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
            present(node, &resident)?;
            crate::colleagues::message(node, resident, to, machine, text, login.as_ref())
                .await
                .map(|(to, landed)| Reply::Spoken { to, landed, delivery: None })
        }
        Call::Say { resident, message, status, attachments } => {
            present(node, &resident)?;
            crate::voice::say(node, &resident, &message, status, &attachments).map(|()| Reply::Told)
        }
        Call::MemorySettings { hub, account, enabled } => {
            users_voice(node, peer)?;
            crate::memory::settings::call(node, hub, account, enabled).await.map(Reply::MemorySettings)
        }
        Call::WatchMemory { .. } => Err(Failure::new("a memory watch is answered as it changes")),
        Call::Memory(request) => {
            let caller = caller::of(node, peer)?;
            crate::memory::api::call(node, &caller, request).await.map(Reply::Memory)
        }
        Call::Project(call) => {
            let caller = caller::of(node, peer)?;
            if !matches!(call, fastctx_types::ProjectCall::Enter { .. }) {
                named_town(node, &caller, call.town()).await?;
            }
            crate::town::project::call(node, &caller, call).await
        }
        Call::Journal(_) => Err(Failure::not_run(fastctx_types::memory::LEGACY)),
        Call::Page(call) => {
            let caller = caller::of(node, peer)?;
            named_town(node, &caller, call.town()).await?;
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
        Call::PeekToken { token } => {
            users_voice(node, peer)?;
            crate::world::peek_token(node, &token).await.map(|peeked| Reply::TokenPeek { peeked })
        }
        Call::OpenAccount { hub, account, password, register, name } => {
            users_voice(node, peer)?;
            crate::world::open_account(node, hub, account, password, register, name).await
        }
        Call::Account { hub, account, change } => {
            users_voice(node, peer)?;
            crate::world::change(node, hub.as_ref(), &account, change).await.map(|token| Reply::Changed { token })
        }
        Call::Accounts => {
            users_voice(node, peer)?;
            Ok(Reply::Accounts(crate::world::accounts(node)))
        }
        Call::RenameHere { to } => {
            users_voice(node, peer)?;
            crate::world::rename_here(node, to).await.map(|()| Reply::Done)
        }
        Call::HubProxy { url, change } => {
            users_voice(node, peer)?;
            if change {
                crate::proxy_setting::set(node, url).await.map(|url| Reply::HubProxy { url })
            } else {
                crate::proxy_setting::kept(&node.world)
                    .map(|url| Reply::HubProxy { url })
                    .map_err(|error| Failure::new(format!("reading the proxy kept: {error}")))
            }
        }
        Call::NetworkUpdates { hub, account, request } => {
            users_voice(node, peer)?;
            crate::world::hub_link(node, Some(&hub))?
                .network_updates(&account, request)
                .await
                .map(Reply::NetworkUpdates)
                .map_err(Failure::new)
        }
        Call::MachineLog { hub, account, before } => {
            users_voice(node, peer)?;
            crate::world::hub_link(node, Some(&hub))?
                .machine_log(&account, before)
                .await
                .map(Reply::MachineLog)
                .map_err(Failure::new)
        }
        Call::ServerCommand { hub, account, name, expires, managed, uses, label, windows, mac } => {
            users_voice(node, peer)?;
            let code = crate::server_invitation::Code { name, expires, managed, uses, label, windows, mac };
            crate::server_invitation::command(node, hub.as_ref(), &account, code)
                .await
                .map(|command| Reply::ServerInvitation { command })
        }
        Call::HubMove { hub, account, request } => {
            users_voice(node, peer)?;
            crate::hub_moving::request(node, hub.as_ref(), &account, request).await.map(Reply::HubMove)
        }
        Call::HubArrived { hub } => {
            users_voice(node, peer)?;
            crate::hub_arrival::installed(node, &hub).await.map(|()| Reply::Done)
        }
        Call::HandOver => {
            users_voice(node, peer)?;
            crate::handoff::hand_over(node.clone());
            Ok(Reply::Done)
        }
        Call::SeenDissolved => {
            users_voice(node, peer)?;
            crate::world::seen_dissolved(node).map(|()| Reply::Done)
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
        Call::Sentinel { hub, account, request } => {
            let caller = caller::of(node, peer)?;
            if hub.is_none()
                && account.is_none()
                && let fastctx_types::sentinel_control::Request::List { owner } = request
            {
                return crate::sentinels::list_all(node, &caller, owner)
                    .await
                    .map(|reply| Reply::Sentinel(Box::new(reply)));
            }
            let (link, account) = crate::sentinels::scope(node, &caller, hub.as_ref(), account.as_ref())?;
            crate::sentinels::control(node, &caller, &link, &account, request)
                .await
                .map(|reply| Reply::Sentinel(Box::new(reply)))
        }
        Call::SentinelSettings { hub, account, change } => {
            if change.is_some() {
                users_voice(node, peer)?;
            }
            let (link, account) =
                crate::profile::scope(node, &caller::of(node, peer)?, hub.as_ref(), account.as_ref())?;
            crate::sentinels::settings(node, &link, &account, change)
                .await
                .map(|reply| Reply::SentinelSettings(Box::new(reply)))
        }
        Call::SentinelKey { hub, account, key, value } => {
            users_voice(node, peer)?;
            let (link, account) =
                crate::profile::scope(node, &caller::of(node, peer)?, hub.as_ref(), account.as_ref())?;
            crate::sentinels::keys::set(node, &link, &account, key, value).await.map(|_| Reply::Done)
        }
        Call::CollabRead { hub, account } => {
            users_voice(node, peer)?;
            match account {
                Some(account) => crate::world::hub_link(node, hub.as_ref())?
                    .collab_read(&account)
                    .map(Reply::CollabRead)
                    .map_err(Failure::new),
                None => {
                    own_settings(hub.as_ref())?;
                    Ok(Reply::CollabRead(fastctx_types::collab::CollabView {
                        collab: crate::own_collab::read(&node.world)?,
                        default_prompt: fastctx_types::collab_prompt::DEFAULT_PROMPT.to_string(),
                    }))
                }
            }
        }
        Call::CollabWrite { hub, account, collab } => {
            users_voice(node, peer)?;
            match account {
                Some(account) => crate::world::hub_link(node, hub.as_ref())?
                    .collab_write(&account, collab)
                    .await
                    .map(|version| Reply::CollabWritten { version })
                    .map_err(Failure::new),
                None => {
                    own_settings(hub.as_ref())?;
                    crate::own_collab::write(node, collab).map(|version| Reply::CollabWritten { version })
                }
            }
        }
        Call::McpRelay { .. } => Err(Failure::not_run("MCP relay requires a dedicated connection")),
        Call::ProfileSubagents { hub, account, operation } => {
            let (link, account) =
                crate::profile::scope(node, &caller::of(node, peer)?, hub.as_ref(), account.as_ref())?;
            link.profile_subagents(&account, operation)
                .await
                .map(|view| Reply::ProfileSubagents(Box::new(view)))
                .map_err(Failure::new)
        }
        Call::ClaudeSubagents { session } => {
            if !matches!(peer, Peer::Session { host } if host.agent.as_str() == "claude") {
                users_voice(node, peer)?;
            }
            crate::profile::subagents::for_session(node, &session)
                .await
                .map(|configuration| Reply::ClaudeSubagents { configuration })
        }
        Call::Plan { hub, account, operation } => {
            crate::plan::call(node, &caller::of(node, peer)?, hub, account, operation)
                .await
                .map(|result| Reply::Plan { result })
        }
        Call::PlanRead { hub, account, owner } => {
            users_voice(node, peer)?;
            let (link, account) =
                crate::profile::scope(node, &caller::of(node, peer)?, hub.as_ref(), account.as_ref())?;
            link.plan_read(&account, owner)
                .await
                .map(|page| Reply::Plan { result: fastctx_types::plan::Result::Page { page } })
                .map_err(Failure::new)
        }
        Call::PlanWrite { hub, account, owner, expected, markdown } => {
            users_voice(node, peer)?;
            let (link, account) =
                crate::profile::scope(node, &caller::of(node, peer)?, hub.as_ref(), account.as_ref())?;
            link.plan_write(&account, owner, expected, markdown, fastctx_types::plan::Author::User)
                .await
                .map(|result| Reply::Plan { result })
                .map_err(Failure::new)
        }
        Call::AccountRecords { hub, account, request } => {
            let caller = caller::of(node, peer)?;
            crate::plan::records_allowed(node, &caller, &request)?;
            let (link, account) = crate::profile::scope(node, &caller, hub.as_ref(), account.as_ref())?;
            link.account_records(&account, request)
                .await
                .map(|response| Reply::AccountRecords { response })
                .map_err(Failure::new)
        }
        Call::WatchAccountRecords { .. } => Err(Failure::new(fastctx_types::account_records::words::WATCH_CONNECTION)),
        Call::ProfileRead { hub, account } => {
            let (link, account) =
                crate::profile::scope(node, &caller::of(node, peer)?, hub.as_ref(), account.as_ref())?;
            link.profile_read(&account).map(|view| Reply::Profile(Box::new(view))).map_err(Failure::new)
        }
        Call::ProfileWrite { hub, account, profile } => {
            let (link, account) =
                crate::profile::scope(node, &caller::of(node, peer)?, hub.as_ref(), account.as_ref())?;
            let current = link.profile_read(&account).map_err(Failure::new)?;
            if profile.servers != current.profile.servers {
                return Err(Failure::not_run("change MCP servers through fastctx mcp"));
            }
            link.profile_write(&account, profile)
                .await
                .map(|version| Reply::ProfileWritten { version })
                .map_err(Failure::new)
        }
        Call::Mcp { hub, account, mcp } => {
            let caller = caller::of(node, peer)?;
            let (link, account) = crate::profile::scope(node, &caller, hub.as_ref(), account.as_ref())?;
            crate::mcp::call(node, link, account, mcp, matches!(caller, Caller::Resident(_)))
                .await
                .map(|result| Reply::Mcp { result })
        }
        Call::Features => crate::features::read(&node.world).map(Reply::Features),
        Call::SetFeature { feature, on } => {
            users_voice(node, peer)?;
            crate::features::set(&node.world, &node.features, feature, on).map(Reply::Features)
        }
        Call::SetLanguage { language } => {
            users_voice(node, peer)?;
            crate::features::speak(&node.world, &node.features, &language).map(Reply::Features)
        }
        Call::WatchFeatures => Err(Failure::new("a features watch is answered as it goes, not at once")),
        Call::WatchWorld => Err(Failure::new("a World watch is answered as it goes, not at once")),
        Call::WorldNow => {
            users_voice(node, peer)?;
            crate::shown::now(node).await.map(|world| Reply::WorldNow(Box::new(world)))
        }
        Call::Reopened { .. } => Err(Failure::new("a way opening is waited for on a connection of its own")),
        Call::Nodes => {
            let caller = caller::of(node, peer)?;
            let acting = crate::around::acting_of(node, &caller);
            let here_too = crate::around::works_here(node, &acting);
            nodes(node, &acting, here_too).await.map(Reply::Nodes)
        }
        Call::Machines { resident, login, thread, scratch } => {
            let explicit = resident.is_some() || thread.is_some();
            let resident =
                resident.or_else(|| thread.as_deref().and_then(|thread| node.residents.resident_of_thread(thread)));
            if explicit && resident.is_none() {
                return Ok(Reply::Machines {
                    here: None,
                    others: Vec::new(),
                    groups: Vec::new(),
                    collab: None,
                    scratch: Default::default(),
                    speaks: false,
                });
            }
            machines(node, resident.as_ref(), login.as_ref(), scratch).await
        }
        Call::WatchTools { .. } => Err(Failure::new("a tools watch is answered as it goes, not at once")),
    }
}

/// Answers once a way from this computer to `machine` opens that was not
/// there when asked (`Call::Reopened`).
async fn reopened(node: &Arc<Node>, machine: crate::reach::Written) -> Result<Reply, Failure> {
    if node.is_here(&machine) {
        return Err(Failure::new(format!("{machine} is this computer; no way to it opens")));
    }
    node.reopened(vec![machine]).await;
    Ok(Reply::Done)
}

/// This computer's own model collaboration belongs to no hub.
fn own_settings(hub: Option<&fastctx_types::HubName>) -> Result<(), Failure> {
    match hub {
        Some(hub) => Err(Failure::not_run(format!(
            "name the account at {hub}; without one, the settings are this computer's own"
        ))),
        None => Ok(()),
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

/// A resident names only the projects of the accounts it works for; any
/// other is answered as one that is not there (`world-network.md`, 一个居民
/// 替谁干活).
async fn named_town(node: &Arc<Node>, caller: &Caller, town: Option<&TownName>) -> Result<(), Failure> {
    let Some(town) = town else { return Ok(()) };
    crate::town::access::ready(node, &crate::around::acting_of(node, caller), town).await
}

/// The town a resident of this machine lives in.
fn town_of(node: &Node, resident: &ResidentId) -> Result<Option<TownName>, Failure> {
    node.residents.active(resident)?;
    crate::town::access::current(node, resident)?;
    node.residents
        .town_of(resident)
        .ok_or_else(|| Failure::not_run(format!("no resident {resident} lives on {}", node.machine())))
}

/// Fails unless `resident` lives here and was not removed.
fn present(node: &Node, resident: &ResidentId) -> Result<(), Failure> {
    town_of(node, resident).map(drop)
}

/// Runs a script for a resident of this machine and records the action in
/// the resident's records. Other machines are reached with `on` inside the
/// script; a session listed before `on` may still name one, and is told so.
async fn run(
    node: &Arc<Node>,
    resident: ResidentId,
    town: Option<TownName>,
    mut request: RunRequest,
    target: Option<crate::reach::Written>,
    call_id: Option<&str>,
) -> Result<RunReply, Failure> {
    request.call_id = call_id.map(str::to_owned);
    request.turn = node.residents.now(&resident).and_then(|now| now.turn);
    let (goes_to, request) = crate::here::start(node, request).map_err(|why| unnamed(node, &resident, &why))?;
    let machine = goes_to.as_ref().map(|place| place.name.clone()).unwrap_or_else(|| node.machine());
    if let Some(id) = call_id {
        crate::run_facts::begin_for(node, &resident, id, machine, request.turn.clone(), &request.script)
            .map_err(|error| Failure::not_run(format!("recording tool call facts: {error}")))?;
    }
    let result = run_started(node, resident.clone(), town, request, goes_to, target, call_id).await;
    // The session is told what became of the script, not of its record.
    if let Err(failure) = &result
        && let Some(id) = call_id
        && let Err(error) = crate::run_facts::failed_for(node, &resident, id, &failure.message)
    {
        node.log.line(format_args!("recording that {resident}'s call {id} failed: {error}"));
    }
    result
}

async fn run_started(
    node: &Arc<Node>,
    resident: ResidentId,
    town: Option<TownName>,
    request: RunRequest,
    goes_to: Option<fastctx_types::access::Written>,
    target: Option<crate::reach::Written>,
    call_id: Option<&str>,
) -> Result<RunReply, Failure> {
    if let Some(target) = target.filter(|target| !node.is_here(target)) {
        return Err(Failure::not_run(format!(
            "run goes to another machine only through `on`: write `on {target}` in the script to run part of it there"
        )));
    }
    let here = node.machine();
    let script = request.script.clone();
    let turn = request.turn.clone();
    let started_at = jiff::Timestamp::now();
    // A script of one `on` goes where it names (`world-run.md`, 这里).
    let source = goes_to.clone().unwrap_or_else(|| here.clone().into());
    let reply = match (goes_to, request) {
        (Some(goes_to), request) => crate::here::run(node, &resident, town.clone(), request, &goes_to).await?,
        (None, request) => {
            // Named like any machine, this one runs it only for whom may work on it.
            let acting = crate::authorization::resident(node, &resident)?;
            node.authorize_here(&acting)?;
            let owner = Address { machine: here.clone(), resident: resident.clone() };
            node.jobs.run(node, owner.into(), town.clone(), request, None, true, None).await?.0
        }
    };
    if let Some(id) = call_id {
        let kept = if let Some(facts) = &reply.run {
            crate::run_facts::keep_reply(node, &resident, id, facts)
        } else {
            // Its script and turn remain known, but missing observations are explicit.
            crate::run_facts::legacy_reply(node, &resident, id, &reply)
        };
        kept.map_err(|error| Failure::new(format!("recording tool call facts: {error}")))?;
    }
    crate::scratch::observed(node, &resident, &source, &reply.scratch)
        .map_err(|error| Failure::new(format!("recording scratch locations: {error}")))?;
    let mut action = crate::jobs::action(&reply, &script, started_at);
    action.turn = turn;
    let world = node.world.clone();
    blocking("recording the action", move || crate::residents::record_action(&world, &resident, &action)).await?;
    node.shown_changed();
    Ok(reply)
}

/// Why a script was not run, with the machines it may name.
fn unnamed(node: &Node, resident: &ResidentId, why: &str) -> Failure {
    let acting = match crate::authorization::resident(node, resident) {
        Ok(acting) => acting,
        Err(failure) => return failure,
    };
    let machines = usable(node, &acting);
    let example = machines.first().map(String::as_str).unwrap_or("MACHINE");
    let names = if machines.is_empty() { "none yet".to_string() } else { machines.join(", ") };
    let why = why.trim_end_matches('.');
    Failure::not_run(format!(
        "nothing was run: {why}. Write the script as one `on MACHINE`, as in `on {example} <<'RUN'` ... `RUN`; the machines you can use: {names}"
    ))
}

/// The machines a session working for `acting` may run on: this one when
/// its accounts may work here, and the others its accounts reach.
fn usable(node: &Node, acting: &fastctx_types::access::Acting) -> Vec<String> {
    let here_too = crate::around::works_here(node, acting);
    let (here, others, _) = crate::world::grouped_machines(node, acting, here_too);
    here_too.then(|| here.to_string()).into_iter().chain(others).collect()
}

/// Writes files for a resident of this machine, from a writing tool a
/// session listed before writing became commands of the shell, and records
/// the write in the resident's records. Files elsewhere are written there.
async fn write(
    node: &Arc<Node>,
    resident: ResidentId,
    town: Option<TownName>,
    op: WriteOp,
    target: Option<crate::reach::Written>,
) -> Result<WriteOutcome, Failure> {
    if let Some(target) = target.filter(|target| !node.is_here(target)) {
        return Err(Failure::new(format!(
            "files on {target} are written there: `on {target} edit PATH` or `on {target} write PATH` in the run tool"
        )));
    }
    // Written here only for an account that may work here, as a run is:
    // an agent assisting another account writes nothing on its owner's
    // computer.
    let acting = crate::authorization::resident(node, &resident)?;
    node.authorize_here(&acting)?;
    let here = node.machine();
    let started_at = jiff::Timestamp::now();
    let turn = node.residents.now(&resident).and_then(|now| now.turn);
    let owner = Address { machine: here.clone(), resident: resident.clone() };
    let caller = Caller::Resident(owner.clone());
    let memory = crate::memory::writing::intercept(node, &caller, &op, None, town.as_ref()).await?;
    let (world, machine, writing, written) = (node.world.clone(), node.names(), town.clone(), op.clone());
    let outcome = match memory {
        Some(outcome) => outcome,
        None => {
            blocking("writing", move || crate::write::write_here(&world, &machine, &owner, writing.as_ref(), written))
                .await?
        }
    };
    let mut action = crate::write::action(&op, &outcome, &here, started_at);
    action.turn = turn;
    let world = node.world.clone();
    blocking("recording the action", move || crate::residents::record_action(&world, &resident, &action)).await?;
    node.shown_changed();
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
    works_here: bool,
    all: bool,
    town: Option<TownName>,
    since: Option<jiff::Timestamp>,
) -> Result<WorldWho, Failure> {
    let here = node.machine();
    let scope = match &town {
        Some(name) => {
            crate::town::access::ready(node, acting, name).await?;
            let (world, town) = (node.world.clone(), name.clone());
            let mut record = blocking("reading the town", move || crate::town::read::latest(&world, &town)).await?;
            if let Some(record) = &mut record {
                crate::ledger::query::seen(node, acting, name, record)
                    .map_err(|error| Failure::new(format!("reading the town {name}: {error}")))?;
            }
            WhoScope::Town { town: name.clone(), record: record.map(Box::new) }
        }
        None => WhoScope::World { towns: crate::ledger::query::list_for(node, acting).await? },
    };
    let mut machines = Vec::new();
    // This computer answers only to whom works on it (`around::works_here`):
    // anyone else is not shown it, not even by name (`world-network.md`,
    // 一个居民替谁干活).
    if works_here || crate::around::looks_here(node, acting) {
        machines.push(match who_here(node, all, town.clone(), since).await {
            Ok(reply) => {
                MachineWho::Answered { observation: None, reply: crate::admission::seen::who_here(node, acting, reply) }
            }
            Err(error) => MachineWho::NoAnswer { observation: None, machine: here.clone(), reason: error.message },
        });
    }
    let reachable = crate::around::reachable_each(node, acting, fastctx_types::access::Purpose::Look);
    let heard = crate::around::ask_each(reachable, |_| RemoteCall::Who { all, town: town.clone(), since }).await;
    machines
        .extend(fastctx_net::around::answers::collect(heard).into_iter().map(|answers| answers.who(since.is_some())));
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
async fn machines(
    node: &Arc<Node>,
    resident: Option<&ResidentId>,
    login: Option<&fastctx_types::collab::Caller>,
    describe_scratch: bool,
) -> Result<Reply, Failure> {
    // A session that names a conversation this computer does not know is
    // told of no machine, never of those of whom the computer is.
    let acting = match resident {
        Some(resident) => match crate::here::acting(node, resident) {
            Some(acting) => acting,
            None => {
                return Ok(Reply::Machines {
                    here: None,
                    others: Vec::new(),
                    groups: Vec::new(),
                    collab: None,
                    scratch: Default::default(),
                    speaks: false,
                });
            }
        },
        None => fastctx_types::access::Acting::Here,
    };
    // This computer is one of them only where the accounts may work here.
    let here_too = crate::around::works_here(node, &acting);
    let (here, others, groups) = crate::world::grouped_machines(node, &acting, here_too);
    let here = here_too.then(|| here.to_string());
    let collab = crate::collab::choices(node, resident, login)?.map(|choices| choices.offered);
    let scratch = match resident.filter(|_| describe_scratch) {
        Some(id) => {
            let paths = crate::scratch::descriptions(
                node,
                id,
                &acting,
                here.iter().cloned().chain(others.iter().cloned()).collect(),
            )
            .await;
            crate::scratch::described(node, id, paths.clone()).map_err(|error| Failure::new(error.to_string()))?;
            paths
        }
        None => Default::default(),
    };
    let speaks = resident.is_some_and(|id| crate::voice::speaks(node, id));
    Ok(Reply::Machines { here, others, groups, collab, scratch, speaks })
}

/// `fastctx nodes`: this computer, and hub by hub the machines whoever asks
/// may work on from here, each asked what it is like when it is online, for
/// one of the accounts it works for. A session sees what it can reach and
/// no more: a hub where it works for no account goes unnamed, and so does
/// this computer for whoever does not work on it (`around::works_here`,
/// `world-network.md`, 一个居民替谁干活).
async fn nodes(
    node: &Arc<Node>,
    acting: &fastctx_types::access::Acting,
    works_here: bool,
) -> Result<NodesReply, Failure> {
    use fastctx_net::around::{self, Heard, Reachable};

    let here = node.machine();
    let links = node
        .links()
        .into_iter()
        .filter(|link| {
            !node.accounts_on(link, acting).is_empty() || matches!(acting, fastctx_types::access::Acting::Here)
        })
        .collect();
    let hubs = around::on_hubs(links, |link, fresh| async move {
        let view = link.view();
        let logins = node.accounts_on(&link, acting);
        // A hub that cannot be asked now still has its machines, as its
        // view last had them (`Link::last_view`).
        let listed = link.last_view().is_some();
        let reachable = view
            .machines()
            .filter(|machine| listed && !view.is_me(&machine.name))
            .filter_map(|machine| {
                view.account_for(&logins, &view.me, &machine.name, fastctx_types::access::Purpose::Hands).map(
                    |account| Reachable {
                        link: link.clone(),
                        name: machine.name.clone(),
                        account,
                        key: machine.key,
                        written: (&machine.name).into(),
                    },
                )
            })
            .collect();
        let online_known = fresh.is_ok();
        let heard = around::ask_refreshed(reachable, |_| RemoteCall::Facts, fresh).await;
        let machines = heard
            .into_iter()
            .map(|(asked, heard)| {
                let name = asked.source.written.name;
                let seen = view
                    .machines()
                    .find(|machine| machine.key.to_string() == asked.source.key)
                    .expect("a machine selected from this view");
                let facts = match heard {
                    Heard::Answered(RemoteReply::Facts(facts)) => Ok(facts),
                    Heard::Answered(_) => Err("it answered something else".to_string()),
                    Heard::Offline(_) => Err("it is offline".to_string()),
                    // With the hub not asked, none of its machines was:
                    // what each said of itself when it last connected.
                    Heard::NoAnswer(_) if !online_known => crate::facts::of(node, &link, seen, None),
                    Heard::NoAnswer(reason) => Err(reason),
                };
                NodeView {
                    written: node.written(&link, &name).to_string(),
                    online: online_known.then_some(seen.online),
                    since: seen.since,
                    // The machine its link goes through is said only when
                    // the asker may work on it too.
                    through: seen.through.clone().filter(|through| {
                        MachineName::try_from(through.clone()).is_ok_and(|through| {
                            (works_here && view.is_me(&through))
                                || view
                                    .account_for(&logins, &view.me, &through, fastctx_types::access::Purpose::Hands)
                                    .is_some()
                        })
                    }),
                    allows_direct: !seen.direct.is_empty(),
                    path: Some(link.path_to(&name)),
                    facts,
                    owner: seen.home.clone(),
                    group: seen.group.clone(),
                    rights: logins.iter().map(|account| (account.clone(), view.rights(account, &name))).collect(),
                    name,
                }
            })
            .collect();
        let here = if works_here {
            logins.iter().map(|account| (account.clone(), view.call_rights(&view.me, account, &view.me))).collect()
        } else {
            Vec::new()
        };
        fastctx_types::world::HubNodes {
            hub: link.membership().id.clone(),
            name: view.hub_name.clone(),
            link: link.state(),
            here,
            logins,
            // Whose this computer is, to whom may work on it alone.
            home: view.me().and_then(|me| me.home.clone()).filter(|_| works_here),
            machines,
        }
    })
    .await;
    let facts = works_here.then(|| crate::world::facts(node));
    Ok(NodesReply { machine: here, facts, hubs })
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

/// Who is on the other end of a connection, as a task answering it says.
fn calling(peer: &Peer) -> String {
    match peer {
        Peer::Page { .. } => "a page's job".to_string(),
        Peer::Command { resident: Some(address), .. } => format!("a command {} ran", address.resident),
        Peer::Command { .. } => "a command on this machine".to_string(),
        Peer::Supervisor { job } => format!("the supervisor of {job}"),
        Peer::Session { .. } => "an agent session's tools".to_string(),
    }
}

fn parse<T: serde::de::DeserializeOwned>(frame: &[u8]) -> std::io::Result<T> {
    serde_json::from_slice(frame).map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
}

fn json(value: &impl serde::Serialize) -> Vec<u8> {
    serde_json::to_vec(value).expect("the World's types always serialize")
}
