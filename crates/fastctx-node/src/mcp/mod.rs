//! Account MCP roster operations and the execution machine's private commands.

mod import;
pub(crate) mod local;
mod process;
pub(crate) mod store;
mod stream;

use crate::Node;
use fastctx_net::link::Link;
use fastctx_types::access::{HubId, Purpose, Written};
use fastctx_types::ipc::Failure;
use fastctx_types::mcp::*;
use fastctx_types::profile::{Entry, Server};
use fastctx_types::remote::{RemoteCall, RemoteReply};
use fastctx_types::{AccountName, MachineName};
use std::collections::BTreeMap;
use std::sync::Arc;

fn failed(error: impl ToString) -> Failure {
    Failure::new(error.to_string())
}

pub(crate) async fn supports(
    _node: &Node,
    link: &Link,
    account: &AccountName,
    machine: &MachineName,
) -> Result<(), Failure> {
    if link.view().is_me(machine) {
        return Ok(());
    }
    match link.call(machine, account, Purpose::Look, RemoteCall::Facts).await.map_err(failed)?? {
        RemoteReply::Facts(facts) if facts.mcp => Ok(()),
        RemoteReply::Facts(_) => Err(Failure::not_run(format!("upgrade {machine} to use user-level MCP servers"))),
        _ => Err(failed(format!("{machine} did not answer its MCP capability"))),
    }
}

fn target(
    node: &Node,
    link: &Link,
    account: &AccountName,
    machine: Option<Written>,
    session: bool,
) -> Result<MachineName, Failure> {
    let view = link.view();
    let name = match machine {
        Some(machine) => {
            if machine.hub.as_ref().is_some_and(|hub| *hub != link.hub_name()) {
                return Err(Failure::not_run("an MCP command belongs to its account's hub"));
            }
            machine.name
        }
        None => {
            let own: Vec<_> = view.machines().filter(|m| view.holds(account, &m.name)).collect();
            if session && own.iter().any(|m| view.is_me(&m.name)) {
                view.me.clone()
            } else if own.len() == 1 {
                own[0].name.clone()
            } else {
                return Err(Failure::not_run("name the machine with --on"));
            }
        }
    };
    view.admits(&view.me, account, &name, Purpose::Hands).map_err(Failure::refused)?;
    let member = view.member(&name).ok_or_else(|| failed(format!("there is no machine {name}")))?;
    if !member.is_machine() {
        return Err(failed(format!("{name} does not run programs")));
    }
    let _ = node;
    Ok(member.name.clone())
}

async fn machine_call(
    node: &Arc<Node>,
    link: &Arc<Link>,
    account: &AccountName,
    machine: &MachineName,
    mcp: MachineCall,
) -> Result<MachineReply, Failure> {
    let view = link.view();
    view.admits(&view.me, account, machine, Purpose::Hands).map_err(Failure::refused)?;
    if view.is_me(machine) {
        return answer(node, &link.membership().id, account, mcp).await;
    }
    if matches!(mcp, MachineCall::Candidates | MachineCall::ImportRemote { .. }) {
        match link.call(machine, account, Purpose::Look, RemoteCall::Facts).await.map_err(failed)?? {
            RemoteReply::Facts(facts) if facts.mcp_import => {}
            _ => return Err(Failure::not_run(format!("upgrade {machine} to import MCP definitions privately"))),
        }
    } else {
        supports(node, link, account, machine).await?;
    }
    match link.call(machine, account, Purpose::Hands, RemoteCall::Mcp { mcp }).await.map_err(failed)?? {
        RemoteReply::Mcp { result: reply } => Ok(reply),
        _ => Err(failed(format!("{machine} answered something other than MCP configuration"))),
    }
}

pub(crate) async fn call(
    node: &Arc<Node>,
    link: Arc<Link>,
    account: AccountName,
    mcp: McpCall,
    session: bool,
) -> Result<McpReply, Failure> {
    let mut profile = link.profile_read(&account).map_err(failed)?;
    if !profile.kept {
        return Err(Failure::not_run(format!("upgrade hub {} to use user-level configuration", link.hub_name())));
    }
    match mcp {
        McpCall::List => {
            let mut states = BTreeMap::new();
            let mut machines: BTreeMap<MachineName, Vec<String>> = BTreeMap::new();
            for (name, entry) in &profile.profile.servers {
                if let Entry::Command { machine } = entry {
                    machines.entry(machine.clone()).or_default().push(name.clone());
                }
            }
            for (machine, names) in machines {
                let view = link.view();
                let online = view.is_me(&machine) || view.member(&machine).is_some_and(|m| m.online);
                let answer = if online {
                    Some(
                        machine_call(node, &link, &account, &machine, MachineCall::Status { names: names.clone() })
                            .await,
                    )
                } else {
                    None
                };
                for name in names {
                    let state = match &answer {
                        None => CommandState::Offline,
                        Some(Ok(MachineReply::Status { kept })) if kept.contains(&name) => CommandState::Kept,
                        Some(Ok(MachineReply::Status { .. })) => CommandState::Missing,
                        Some(Err(e)) if e.message.starts_with("upgrade ") => CommandState::Older,
                        Some(Err(e)) => CommandState::Failed { problem: e.message.clone() },
                        Some(_) => CommandState::Failed { problem: "the machine gave an unexpected answer".into() },
                    };
                    states.insert(name, state);
                }
            }
            Ok(McpReply::Listed {
                servers: profile
                    .profile
                    .servers
                    .into_iter()
                    .map(|(name, entry)| {
                        let server = match entry {
                            Entry::Remote(remote) => ListedServer::Remote(remote),
                            Entry::Command { machine } => {
                                ListedServer::Command { machine, state: states.remove(&name).expect("queried command") }
                            }
                            Entry::Unknown => ListedServer::Unknown,
                        };
                        Listed { name, server }
                    })
                    .collect(),
            })
        }
        McpCall::Keep { name, server, machine } => {
            fastctx_types::profile::name_of(&name).map_err(failed)?;
            let command = match server {
                Server::Remote(remote) => {
                    profile.profile.servers.insert(name.clone(), Entry::Remote(remote));
                    None
                }
                Server::Command(server) => {
                    let machine = target(node, &link, &account, machine, session)?;
                    supports(node, &link, &account, &machine).await?;
                    profile.profile.servers.insert(name.clone(), Entry::Command { machine: machine.clone() });
                    Some((machine, server))
                }
            };
            let version = link.profile_write(&account, profile.profile).await.map_err(failed)?;
            if let Some((machine, server)) = command {
                machine_call(node,&link,&account,&machine,MachineCall::Keep { name: name.clone(),version,server }).await.map_err(|e| failed(format!("{name} is registered at profile version {version}, but its command was not confirmed on {machine}: {e}; add it again")))?;
            }
            Ok(McpReply::Kept { name, version })
        }
        McpCall::Remove { name } => {
            if profile.profile.servers.remove(&name).is_none() {
                return Err(failed(format!("there is no MCP server {name}")));
            }
            let version = link.profile_write(&account, profile.profile).await.map_err(failed)?;
            Ok(McpReply::Removed { version })
        }
        McpCall::Found { machine } => {
            let machine = target(
                node,
                &link,
                &account,
                machine.or_else(|| Some(Written { name: node.machine(), hub: Some(link.hub_name()) })),
                session,
            )?;
            let MachineReply::Found { servers } =
                machine_call(node, &link, &account, &machine, MachineCall::Found).await?
            else {
                return Err(failed("the machine did not list import candidates"));
            };
            if servers.iter().any(|server| matches!(server.server, FoundServer::Remote(_))) {
                return Err(failed("upgrade the requesting Fastctx to list MCP import candidates privately"));
            }
            Ok(McpReply::Found { machine, servers })
        }
        McpCall::Candidates { machine } => {
            let machine = target(
                node,
                &link,
                &account,
                machine.or_else(|| Some(Written { name: node.machine(), hub: Some(link.hub_name()) })),
                session,
            )?;
            let MachineReply::Candidates { servers } =
                machine_call(node, &link, &account, &machine, MachineCall::Candidates).await?
            else {
                return Err(failed("the machine did not list private import candidates"));
            };
            Ok(McpReply::Candidates { machine, servers })
        }
        McpCall::Import { machine, source, name, as_name } => {
            fastctx_types::profile::name_of(&as_name).map_err(failed)?;
            let machine = target(node, &link, &account, Some(machine), session)?;
            let MachineReply::Candidates { servers } =
                machine_call(node, &link, &account, &machine, MachineCall::Candidates).await?
            else {
                return Err(failed("the machine did not list private import candidates"));
            };
            let found = servers
                .into_iter()
                .find(|f| f.source == source && f.name == name)
                .ok_or_else(|| failed("the selected MCP configuration no longer exists"))?;
            let command = found.kind == ImportKind::Command;
            let entry = if command {
                Entry::Command { machine: machine.clone() }
            } else {
                let MachineReply::ImportedRemote { server } = machine_call(
                    node,
                    &link,
                    &account,
                    &machine,
                    MachineCall::ImportRemote { source: source.clone(), original: name.clone() },
                )
                .await?
                else {
                    return Err(failed("the machine did not reread the selected URL server"));
                };
                Entry::Remote(server)
            };
            profile.profile.servers.insert(as_name.clone(), entry);
            let version = link.profile_write(&account, profile.profile).await.map_err(failed)?;
            if command {
                machine_call(
                    node,
                    &link,
                    &account,
                    &machine,
                    MachineCall::Import { name: as_name.clone(), version, source, original: name },
                )
                .await?;
            }
            Ok(McpReply::Kept { name: as_name, version })
        }
        McpCall::Test { name } => match profile.profile.servers.get(&name) {
            Some(Entry::Command { machine }) => {
                match machine_call(node, &link, &account, machine, MachineCall::Test { name }).await? {
                    MachineReply::Tested(tested) => Ok(McpReply::Tested(tested)),
                    _ => Err(failed("the machine did not answer the MCP test")),
                }
            }
            Some(Entry::Remote(remote)) => Ok(McpReply::Tested(fastctx_net::mcp_test::test(remote).await)),
            _ => Err(failed(format!("there is no usable MCP server {name}"))),
        },
    }
}

pub(crate) async fn answer(
    node: &Arc<Node>,
    hub: &HubId,
    account: &AccountName,
    call: MachineCall,
) -> Result<MachineReply, Failure> {
    match call {
        MachineCall::Status { names } => {
            let kept = store::status(node, hub, account, &names).map_err(failed)?;
            Ok(MachineReply::Status { kept })
        }
        MachineCall::Keep { name, version, server } => {
            store::keep(node, hub, account, name, version, server).await.map_err(failed)?;
            Ok(MachineReply::Kept)
        }
        MachineCall::Found => Ok(MachineReply::Found { servers: import::legacy(node).map_err(failed)? }),
        MachineCall::Candidates => Ok(MachineReply::Candidates { servers: import::metadata(node).map_err(failed)? }),
        MachineCall::ImportRemote { source, original } => {
            let server = import::selected(node, &source, &original).map_err(failed)?;
            match server {
                Server::Remote(server) => Ok(MachineReply::ImportedRemote { server }),
                Server::Command(_) => Err(failed("the selected MCP configuration is no longer a URL server")),
            }
        }
        MachineCall::Import { name, version, source, original } => {
            let server = import::selected(node, &source, &original).map_err(failed)?;
            let Server::Command(server) = server else {
                return Err(failed("the import candidate is no longer a command"));
            };
            store::keep(node, hub, account, name, version, server).await.map_err(failed)?;
            Ok(MachineReply::Kept)
        }
        MachineCall::Test { name } => Ok(MachineReply::Tested(test(node, hub, account, &name).await)),
    }
}

async fn test(node: &Node, hub: &HubId, account: &AccountName, name: &str) -> Tested {
    let mut tools = Vec::new();
    let mut process =
        match store::get(node, hub, account, name).and_then(|s| process::start(&s, &node.fastctx, &node.root)) {
            Ok(p) => p,
            Err(problem) => return Tested { tools, problem: Some(problem), log: None },
        };
    let trial = async {
        use serde_json::json;
        for (id, method, params) in [
            (
                1,
                "initialize",
                json!({"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"fastctx","version":env!("CARGO_PKG_VERSION")}}),
            ),
            (2, "tools/list", json!({})),
        ] {
            process
                .input
                .send(serde_json::to_vec(&json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})).unwrap())
                .await
                .map_err(|_| "the MCP stopped".to_string())?;
            loop {
                let line = process.output.recv().await.ok_or("the MCP closed its output")??;
                let reply = fastctx_types::mcp_message::parse(&line)?;
                if reply.get("id") != Some(&json!(id)) {
                    continue;
                }
                if let Some(error) = reply.get("error") {
                    return Err(error.to_string());
                }
                if id == 2 {
                    tools = reply["result"]["tools"]
                        .as_array()
                        .ok_or("tools/list returned no tools array")?
                        .iter()
                        .filter_map(|t| t["name"].as_str().map(str::to_string))
                        .collect();
                }
                break;
            }
            if id == 1 {
                process
                    .input
                    .send(br#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#.to_vec())
                    .await
                    .map_err(|_| "the MCP stopped".to_string())?;
            }
        }
        Ok::<(), String>(())
    };
    let result = tokio::time::timeout(std::time::Duration::from_secs(60), trial)
        .await
        .map_err(|_| "the MCP did not initialize and list tools within 60 s".to_string())
        .and_then(|r| r);
    let log = String::from_utf8_lossy(&process.log.lock().expect("never poisoned")).into_owned();
    let _ = store::keep_log(node, hub, account, name, log.as_bytes());
    Tested { tools, problem: result.err(), log: (!log.is_empty()).then_some(log) }
}

pub(crate) async fn serve(
    node: &Arc<Node>,
    hub: &HubId,
    account: &AccountName,
    name: &str,
    stream: fastctx_net::data::Stream,
) -> Result<(), Failure> {
    let mut changes = node.views.subscribe();
    let mut ways = node.ways.subscribe();
    let acting = fastctx_types::access::Acting::one(hub.clone(), account.clone());
    node.authorize_here(&acting)?;
    let server = store::get(node, hub, account, name).map_err(failed)?;
    let mut process = process::start(&server, &node.fastctx, &node.root).map_err(failed)?;
    let revoked = async {
        loop {
            node.authorize_here(&acting)?;
            if store::get(node, hub, account, name).as_ref() != Ok(&server) {
                return Err(failed("the MCP command was changed or removed"));
            }
            tokio::select! {
                changed = changes.changed() => changed.map_err(failed)?,
                changed = ways.changed() => changed.map_err(failed)?,
            }
        }
    };
    let result = tokio::select! { result = stream::serve(&mut process,stream) => result.map_err(failed), result = revoked => result };
    let log = process.log.lock().expect("never poisoned").clone();
    if let Err(error) = store::keep_log(node, hub, account, name, &log) {
        node.log.line(format_args!("keeping MCP diagnostics: {error}"));
    }
    result
}
