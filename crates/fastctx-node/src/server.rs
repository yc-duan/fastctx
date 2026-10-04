//! One client's connection: the welcome, then its calls one at a time.

use std::sync::Arc;

use fastctx_net::link::CallFailure;
use fastctx_platform::ipc::Connection;
use fastctx_types::ipc::{ClientFrame, Failure, Hello, NodeFrame, PROTOCOL, Peer, Welcome};
use fastctx_types::remote::{RemoteCall, RemoteReply};
use fastctx_types::run::{RunReply, RunRequest};
use fastctx_types::town::{MachineWho, WhoReply, WorldWho};
use fastctx_types::world::{Node as NodeView, NodesReply, Revoked};
use fastctx_types::write::{WriteOp, WriteOutcome};
use fastctx_types::{Address, Call, MachineName, Reply, ResidentId, TownName};

use crate::Node;

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
    // closes, however it closes.
    let mut arrived = Arrived { node, residents: Vec::new() };
    while let Some(frame) = reader.recv().await? {
        let ClientFrame::Call { id, call } = parse(&frame)?;
        let arriving = matches!(call, Call::Arrive { .. });
        let reply = handle(node, &hello.peer, call).await;
        if arriving && let Ok(Reply::Arrived(whereabouts)) = &reply {
            arrived.residents.push(whereabouts.resident.clone());
        }
        writer.send(&json(&NodeFrame::Reply { id, reply })).await?;
    }
    Ok(())
}

struct Arrived<'a> {
    node: &'a Node,
    residents: Vec<ResidentId>,
}

impl Drop for Arrived<'_> {
    fn drop(&mut self) {
        self.node.residents.left(&self.residents);
    }
}

async fn handle(node: &Arc<Node>, _peer: &Peer, call: Call) -> Result<Reply, Failure> {
    match call {
        Call::Arrive { host, hints } => {
            node.residents.arrive(&node.world, &node.machine(), host, hints).map(Reply::Arrived)
        }
        Call::Run { resident, town, request, node: target } => {
            run(node, resident, town, request, target).await.map(Reply::Run)
        }
        Call::Write { resident, town, op, node: target } => {
            write(node, resident, town, op, target).await.map(|outcome| Reply::Written { outcome })
        }
        Call::JobChanged { job, owner, town } => {
            node.jobs.changed(&node.world, &node.machine(), job, owner, town, node.link()).await.map(|()| Reply::Done)
        }
        Call::Saw { owner, town, seen } => {
            let (world, here) = (node.world.clone(), node.machine());
            blocking("recording what was seen", move || {
                crate::residents::record_seen(&world, &here, &owner, town.as_ref(), &seen)
            })
            .await
            .map(|()| Reply::Done)
        }
        Call::Who { all } => who(node, all).await.map(Reply::Who),
        Call::Collect { resident, town } => {
            let world = node.world.clone();
            blocking("reading the inbox", move || crate::inbox::collect(&world, &resident, town.as_ref()))
                .await
                .map(|notices| Reply::Notices { notices })
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
            node.jobs.run(&node.world, &here, owner, town.clone(), request, node.link()).await?.0
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

/// This machine's own answer to `fastctx w`.
pub async fn who_here(node: &Node, all: bool) -> Result<WhoReply, Failure> {
    let everyone = node.residents.everyone();
    let (world, machine) = (node.world.clone(), node.machine());
    blocking("looking around", move || crate::who::who(&world, &machine, everyone, all)).await
}

/// `fastctx w` across the World: this machine, and every member that is
/// connected, asked at once.
async fn who(node: &Arc<Node>, all: bool) -> Result<WorldWho, Failure> {
    let mut machines = vec![MachineWho::Answered { reply: who_here(node, all).await? }];
    if let Some(link) = node.link() {
        // The list of members and who is connected, as the hub has it now.
        let refreshed = link.refresh().await;
        let (table, presence) = link.members();
        let me = link.membership().name.clone();
        let mut asks = Vec::new();
        for member in table.members().filter(|member| member.name != me) {
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
                    let (link, name) = (link.clone(), member.name.clone());
                    asks.push(tokio::spawn(async move {
                        match link.call(&name, RemoteCall::Who { all }).await {
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
    Ok(WorldWho { machine: node.machine(), machines })
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
