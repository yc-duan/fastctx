//! Configure MCP servers for the account the caller works for.

use clap::{Args, Subcommand};
use fastctx_types::mcp::*;
use fastctx_types::profile::{self, CommandMcp, RemoteMcp, Server};
use fastctx_types::{AccountName, Call, HubName, Reply, access::Written};

#[derive(Args)]
pub(crate) struct McpArgs {
    #[arg(long = "as", global = true)]
    account: Option<AccountName>,
    #[arg(long, global = true)]
    hub: Option<HubName>,
    #[command(subcommand)]
    command: McpCommand,
}
#[derive(Subcommand)]
enum McpCommand {
    List,
    Add {
        name: Option<String>,
        #[arg(long)]
        on: Option<Written>,
        #[arg(long,conflicts_with_all=["url","command"])]
        json: Option<String>,
        #[arg(long, conflicts_with = "command")]
        url: Option<String>,
        #[arg(long, requires = "url")]
        sse: bool,
        #[arg(long, requires = "url")]
        header: Vec<String>,
        #[arg(long)]
        env: Vec<String>,
        #[arg(long)]
        cwd: Option<String>,
        #[arg(last = true)]
        command: Vec<String>,
    },
    /// List existing configurations; select their numbered entries to import.
    Import {
        #[arg(long)]
        on: Option<Written>,
        #[arg(long, value_delimiter = ',')]
        select: Vec<usize>,
    },
    Test {
        name: String,
    },
    Remove {
        name: String,
    },
}
fn pairs(values: Vec<String>) -> anyhow::Result<std::collections::BTreeMap<String, String>> {
    values
        .into_iter()
        .map(|value| {
            value
                .split_once('=')
                .map(|(k, v)| (k.into(), v.into()))
                .ok_or_else(|| anyhow::anyhow!("expected NAME=VALUE"))
        })
        .collect()
}
pub(crate) fn run(args: McpArgs) -> anyhow::Result<()> {
    let ask = |mcp| -> anyhow::Result<McpReply> {
        match crate::ask::call(Call::Mcp { hub: args.hub.clone(), account: args.account.clone(), mcp })?.0 {
            Reply::Mcp { result: reply } => Ok(reply),
            _ => anyhow::bail!("the node did not answer the MCP call"),
        }
    };
    match args.command {
        McpCommand::List => show(ask(McpCall::List)?)?,
        McpCommand::Test { name } => show(ask(McpCall::Test { name })?)?,
        McpCommand::Remove { name } => show(ask(McpCall::Remove { name })?)?,
        McpCommand::Add { name, on, json, url, sse, header, env, cwd, command } => {
            let servers = if let Some(mut text) = json {
                if text == "-" {
                    use std::io::Read as _;
                    text.clear();
                    std::io::stdin().read_to_string(&mut text)?;
                }
                profile::parse_servers(&text).map_err(anyhow::Error::msg)?
            } else {
                let name = name.ok_or_else(|| anyhow::anyhow!("name the MCP server"))?;
                let server = if let Some(url) = url {
                    Server::Remote(RemoteMcp { url, sse, headers: pairs(header)? })
                } else {
                    let (program, args) =
                        command.split_first().ok_or_else(|| anyhow::anyhow!("give --url or -- PROGRAM ARGUMENTS"))?;
                    Server::Command(CommandMcp { command: program.clone(), args: args.to_vec(), env: pairs(env)?, cwd })
                };
                vec![(name, server)]
            };
            for (name, server) in servers {
                show(ask(McpCall::Keep { name, server, machine: on.clone() })?)?;
            }
        }
        McpCommand::Import { on, select } => {
            let McpReply::Found { machine, servers } = ask(McpCall::Found { machine: on.clone() })? else {
                anyhow::bail!("the machine did not list import candidates");
            };
            if select.is_empty() {
                for (index, server) in servers.iter().enumerate() {
                    println!(
                        "{}: {} on {} ({:?}, {})",
                        index + 1,
                        server.name,
                        machine,
                        server.source,
                        match server.server {
                            FoundServer::Command => "command",
                            FoundServer::Remote(_) => "URL",
                        }
                    );
                }
            } else {
                let McpReply::Listed { servers: listed } = ask(McpCall::List)? else {
                    anyhow::bail!("the node did not list MCP servers");
                };
                let mut names: std::collections::BTreeSet<_> = listed.into_iter().map(|s| s.name).collect();
                for index in select {
                    let found = servers
                        .get(index.wrapping_sub(1))
                        .ok_or_else(|| anyhow::anyhow!("no import candidate {index}"))?;
                    let name = profile::free_name(&found.name, machine.as_str(), |name| names.contains(name));
                    names.insert(name.clone());
                    show(ask(McpCall::Import {
                        machine: Written { name: machine.clone(), hub: on.as_ref().and_then(|m| m.hub.clone()) },
                        source: found.source.clone(),
                        name: found.name.clone(),
                        as_name: name,
                    })?)?;
                }
            }
        }
    }
    Ok(())
}
fn show(reply: McpReply) -> anyhow::Result<()> {
    match reply {
        McpReply::Listed { servers } => {
            for server in servers {
                match server.server {
                    ListedServer::Remote(remote) => {
                        println!("{}: URL{}", server.name, if remote.sse { " (SSE; unavailable in Codex)" } else { "" })
                    }
                    ListedServer::Command { machine, state } => println!(
                        "{}: {} — {}",
                        server.name,
                        machine,
                        match state {
                            CommandState::Kept => "stored".into(),
                            CommandState::Offline => "offline".into(),
                            CommandState::Missing => "command missing; add it again".into(),
                            CommandState::Older => "upgrade this machine".into(),
                            CommandState::Failed { problem } => problem,
                        }
                    ),
                    ListedServer::Unknown => println!("{}: upgrade Fastctx to read this kind", server.name),
                }
            }
        }
        McpReply::Kept { name, version } => println!("{name}: stored at profile version {version}"),
        McpReply::Removed { version } => println!("removed at profile version {version}"),
        McpReply::Tested(tested) => {
            for tool in tested.tools {
                println!("{tool}");
            }
            if let Some(problem) = tested.problem {
                if let Some(log) = tested.log {
                    eprintln!("{log}");
                }
                anyhow::bail!("{problem}");
            }
            println!("MCP initialized and listed its tools");
        }
        McpReply::Found { .. } => anyhow::bail!("unexpected import answer"),
    }
    Ok(())
}
