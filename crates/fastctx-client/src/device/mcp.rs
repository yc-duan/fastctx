//! Devices edit the hub roster directly, including while every computer is
//! offline. Command details go only to the selected execution machine.

use super::failed;
use crate::Error;
use fastctx_net::link::Link;
use fastctx_types::{
    AccountName, MachineName,
    access::{Purpose, Written},
    mcp::*,
    profile::{Entry, Server},
    remote::{RemoteCall, RemoteReply},
};

pub(super) async fn supports(link: &Link, account: &AccountName, machine: &MachineName) -> Result<(), Error> {
    match link.call(machine, account, Purpose::Look, RemoteCall::Facts).await.map_err(failed)?.map_err(failed)? {
        RemoteReply::Facts(facts) if facts.mcp => Ok(()),
        _ => Err(failed(format!("upgrade {machine} to configure MCP servers"))),
    }
}
async fn ask(
    link: &Link,
    account: &AccountName,
    machine: &MachineName,
    mcp: MachineCall,
) -> Result<MachineReply, Error> {
    if matches!(mcp, MachineCall::Candidates | MachineCall::ImportRemote { .. }) {
        match link.call(machine, account, Purpose::Look, RemoteCall::Facts).await.map_err(failed)?.map_err(failed)? {
            RemoteReply::Facts(facts) if facts.mcp_import => {}
            _ => return Err(failed(format!("upgrade {machine} to import MCP definitions privately"))),
        }
    } else {
        supports(link, account, machine).await?;
    }
    match link.call(machine, account, Purpose::Hands, RemoteCall::Mcp { mcp }).await.map_err(failed)?.map_err(failed)? {
        RemoteReply::Mcp { result } => Ok(result),
        _ => Err(failed(format!("{machine} did not answer the MCP call"))),
    }
}
fn target(link: &Link, account: &AccountName, given: Option<Written>) -> Result<MachineName, Error> {
    let view = link.view();
    let name = match given {
        Some(written) => {
            if written.hub.is_some_and(|hub| hub != link.hub_name()) {
                return Err(failed("choose a machine on the account's hub"));
            }
            written.name
        }
        None => {
            let own: Vec<_> = view.machines().filter(|m| view.holds(account, &m.name)).collect();
            if own.len() != 1 {
                return Err(failed("choose the MCP's machine"));
            }
            own[0].name.clone()
        }
    };
    view.admits(&view.me, account, &name, Purpose::Hands).map_err(failed)?;
    let member = view.member(&name).ok_or_else(|| failed(format!("there is no machine {name}")))?;
    if !member.is_machine() {
        return Err(failed(format!("{name} does not run programs")));
    }
    Ok(member.name.clone())
}

pub(super) async fn call(link: &Link, account: &AccountName, mcp: McpCall) -> Result<McpReply, Error> {
    let mut view = link.profile_read(account).map_err(failed)?;
    if !view.kept {
        return Err(failed(format!("upgrade hub {} to use user-level configuration", link.hub_name())));
    }
    match mcp {
        McpCall::List => {
            let mut servers = Vec::new();
            for (name, entry) in view.profile.servers {
                let server = match entry {
                    Entry::Remote(remote) => ListedServer::Remote(remote),
                    Entry::Unknown => ListedServer::Unknown,
                    Entry::Command { machine } => {
                        let state = if !link.view().member(&machine).is_some_and(|m| m.online) {
                            CommandState::Offline
                        } else {
                            match ask(link, account, &machine, MachineCall::Status { names: vec![name.clone()] }).await
                            {
                                Ok(MachineReply::Status { kept }) if kept.contains(&name) => CommandState::Kept,
                                Ok(MachineReply::Status { .. }) => CommandState::Missing,
                                Err(e) if e.to_string().contains("upgrade ") => CommandState::Older,
                                Err(e) => CommandState::Failed { problem: e.to_string() },
                                _ => CommandState::Failed { problem: "unexpected MCP status".into() },
                            }
                        };
                        ListedServer::Command { machine, state }
                    }
                };
                servers.push(Listed { name, server });
            }
            Ok(McpReply::Listed { servers })
        }
        McpCall::Remove { name } => {
            if view.profile.servers.remove(&name).is_none() {
                return Err(failed(format!("there is no MCP server {name}")));
            }
            Ok(McpReply::Removed { version: link.profile_write(account, view.profile).await.map_err(failed)? })
        }
        McpCall::Keep { name, server, machine } => {
            fastctx_types::profile::name_of(&name).map_err(failed)?;
            let command = match server {
                Server::Remote(remote) => {
                    view.profile.servers.insert(name.clone(), Entry::Remote(remote));
                    None
                }
                Server::Command(command) => {
                    let machine = target(link, account, machine)?;
                    supports(link, account, &machine).await?;
                    view.profile.servers.insert(name.clone(), Entry::Command { machine: machine.clone() });
                    Some((machine, command))
                }
            };
            let version = link.profile_write(account, view.profile).await.map_err(failed)?;
            if let Some((machine, server)) = command {
                ask(link,account,&machine,MachineCall::Keep {name:name.clone(),version,server}).await.map_err(|e|failed(format!("{name} is registered at version {version}, but its command was not confirmed on {machine}: {e}; add it again")))?;
            }
            Ok(McpReply::Kept { name, version })
        }
        McpCall::Found { machine } => {
            let machine = target(link, account, machine)?;
            let MachineReply::Found { servers } = ask(link, account, &machine, MachineCall::Found).await? else {
                return Err(failed("the machine did not list import candidates"));
            };
            if servers.iter().any(|server| matches!(server.server, FoundServer::Remote(_))) {
                return Err(failed("upgrade the requesting Fastctx to list MCP import candidates privately"));
            }
            Ok(McpReply::Found { machine, servers })
        }
        McpCall::Candidates { machine } => {
            let machine = target(link, account, machine)?;
            let MachineReply::Candidates { servers } = ask(link, account, &machine, MachineCall::Candidates).await?
            else {
                return Err(failed("the machine did not list private import candidates"));
            };
            Ok(McpReply::Candidates { machine, servers })
        }
        McpCall::Import { machine, source, name, as_name } => {
            fastctx_types::profile::name_of(&as_name).map_err(failed)?;
            let machine = target(link, account, Some(machine))?;
            let MachineReply::Candidates { servers } = ask(link, account, &machine, MachineCall::Candidates).await?
            else {
                return Err(failed("the machine did not list private import candidates"));
            };
            let found = servers
                .into_iter()
                .find(|f| f.source == source && f.name == name)
                .ok_or_else(|| failed("the import candidate disappeared"))?;
            let command = found.kind == ImportKind::Command;
            let entry = if command {
                Entry::Command { machine: machine.clone() }
            } else {
                let MachineReply::ImportedRemote { server } = ask(
                    link,
                    account,
                    &machine,
                    MachineCall::ImportRemote { source: source.clone(), original: name.clone() },
                )
                .await?
                else {
                    return Err(failed("the machine did not reread the selected URL server"));
                };
                Entry::Remote(server)
            };
            view.profile.servers.insert(as_name.clone(), entry);
            let version = link.profile_write(account, view.profile).await.map_err(failed)?;
            if command {
                ask(
                    link,
                    account,
                    &machine,
                    MachineCall::Import { name: as_name.clone(), version, source, original: name },
                )
                .await?;
            }
            Ok(McpReply::Kept { name: as_name, version })
        }
        McpCall::Test { name } => match view.profile.servers.get(&name) {
            Some(Entry::Remote(remote)) => Ok(McpReply::Tested(fastctx_net::mcp_test::test(remote).await)),
            Some(Entry::Command { machine }) => match ask(link, account, machine, MachineCall::Test { name }).await? {
                MachineReply::Tested(tested) => Ok(McpReply::Tested(tested)),
                _ => Err(failed("the machine did not answer the MCP test")),
            },
            _ => Err(failed(format!("there is no usable MCP server {name}"))),
        },
    }
}
