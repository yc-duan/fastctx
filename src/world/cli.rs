//! `fastctx hub …`, `fastctx world …`, and `fastctx node …`: the World control commands.

use super::NetworkMode;
use super::node::admin::{AdminRequest, AdminResponse};
use crate::control::paths::ControlPaths;
use clap::{Subcommand, ValueEnum};
use std::path::PathBuf;
use std::process::ExitCode;

/// Hub-side commands, run on the machine that hosts the hub.
#[derive(Debug, Subcommand)]
pub enum HubCommand {
    /// Run the hub in the foreground until interrupted.
    Serve {
        /// Address to listen on; 443 looks like an ordinary web server (Linux needs `setcap`).
        #[arg(long, value_name = "HOST:PORT", default_value = "0.0.0.0:443")]
        listen: String,
        /// Hub data directory (database, key, certificate, status).
        #[arg(long, value_name = "DIR")]
        data: Option<PathBuf>,
        /// PEM certificate chain for a publicly trusted identity; with --key.
        #[arg(long, value_name = "FILE", requires = "key")]
        cert: Option<PathBuf>,
        /// PEM private key matching --cert.
        #[arg(long, value_name = "FILE", requires = "cert")]
        key: Option<PathBuf>,
        /// Serve plain HTTP for a reverse proxy or CDN that terminates TLS in front of the hub.
        #[arg(long, conflicts_with_all = ["cert", "key"])]
        behind_proxy: bool,
        /// Discard the unused bootstrap password and print a new one.
        #[arg(long)]
        reset_bootstrap: bool,
    },
    /// Show the running hub's status from its status file.
    Status {
        /// Hub data directory.
        #[arg(long, value_name = "DIR")]
        data: Option<PathBuf>,
    },
    /// Remove a member's admission.
    Revoke {
        /// Member name.
        name: String,
        /// Hub data directory.
        #[arg(long, value_name = "DIR")]
        data: Option<PathBuf>,
    },
    /// Make the hub itself a member so a web AI can use the World through it.
    Join {
        /// Invite string from `fastctx world invite`.
        invite: String,
    },
}

/// Network path selection for the hub link.
#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum NetworkChoice {
    /// Physical interface first, then the operating system's routing and proxy.
    Auto,
    /// Pin to a physical interface; bypass proxies, TUN adapters, and system DNS.
    Direct,
    /// Use the operating system's routing, resolver, and HTTPS_PROXY.
    System,
}

impl From<NetworkChoice> for NetworkMode {
    fn from(choice: NetworkChoice) -> Self {
        match choice {
            NetworkChoice::Auto => Self::Auto,
            NetworkChoice::Direct => Self::Direct,
            NetworkChoice::System => Self::System,
        }
    }
}

/// World-wide commands, run on any member machine.
#[derive(Debug, Subcommand)]
pub enum WorldCommand {
    /// Send a resident network operation as JSON on stdin; emit a JSON response.
    Network,
    /// Create the World from this machine: the founding member.
    Init {
        /// Hub address, `host:port` (port 443 when omitted).
        hub: String,
        /// The one-time bootstrap password the hub printed at first start.
        #[arg(long, value_name = "PASSWORD", allow_hyphen_values = true)]
        bootstrap: String,
        /// This machine's name in the World: lowercase letters, digits, hyphens.
        #[arg(long, value_name = "NAME")]
        name: String,
        /// Tags for selectors like `tag:office`; repeatable.
        #[arg(long = "tag", value_name = "TAG")]
        tags: Vec<String>,
        /// Network path for the hub link.
        #[arg(long, value_enum, default_value = "auto")]
        network: NetworkChoice,
        /// Physical interface to pin the direct path to.
        #[arg(long, value_name = "NAME")]
        interface: Option<String>,
        /// Write the enrollment but do not install or start the node service.
        #[arg(long)]
        no_service: bool,
    },
    /// Print a pasteable invite for a new machine.
    Invite {
        /// Suggested name for the new machine.
        #[arg(long, value_name = "NAME")]
        name: Option<String>,
        /// Hours until the invite expires.
        #[arg(long, default_value_t = 24)]
        hours: u32,
        /// Hub addresses to put in the invite instead of this machine's; repeatable.
        #[arg(long = "hub", value_name = "HOST:PORT")]
        hubs: Vec<String>,
    },
    /// Revoke a member with this device's signature.
    Revoke {
        /// Member name.
        name: String,
    },
    /// Show or change the grants that decide who may use which verbs on which machines.
    Grants {
        #[command(subcommand)]
        command: Option<GrantsCommand>,
    },
    /// Show the machines in the World.
    Nodes {
        /// Print JSON instead of the table.
        #[arg(long)]
        json: bool,
        /// Print one machine name per line, for scripts.
        #[arg(long, conflicts_with = "json")]
        names: bool,
        /// Only machines that are online now.
        #[arg(long)]
        online: bool,
        /// Only machines running this operating system: linux, macos, or windows.
        #[arg(long, value_name = "OS")]
        os: Option<String>,
        /// Only machines carrying this tag; repeatable, every tag must match.
        #[arg(long = "tag", value_name = "TAG")]
        tags: Vec<String>,
    },
    /// Run a bash command on one machine and wait for it: the door scripts use.
    Run {
        /// The machine, as listed by `fastctx world nodes`.
        #[arg(long, value_name = "MACHINE")]
        node: String,
        /// Working directory on that machine. Omitted: its home directory, or the current
        /// directory when the machine is this one.
        #[arg(long, value_name = "DIR")]
        cwd: Option<String>,
        /// How long to wait, like `90s`, `10m`, or `2h`; 24h at most.
        #[arg(long, value_name = "DURATION", default_value = "1h")]
        timeout: String,
        /// Print `{node, status, exit_code, text}` as JSON instead of the tool's text.
        #[arg(long)]
        json: bool,
        /// The bash command line; put `--` before it when it starts with a dash.
        #[arg(required = true, trailing_var_arg = true, value_name = "COMMAND")]
        command: Vec<String>,
    },
    /// Copy a file or a directory tree between two machines: identical files are skipped,
    /// differing ones replaced, nothing is deleted.
    Copy {
        /// Machine holding FROM; this machine when omitted.
        #[arg(long, value_name = "MACHINE")]
        from_node: Option<String>,
        /// Machine receiving the copy; this machine when omitted.
        #[arg(long, value_name = "MACHINE")]
        to_node: Option<String>,
        /// How long to wait, like `10m` or `2h`; 24h at most.
        #[arg(long, value_name = "DURATION", default_value = "1h")]
        timeout: String,
        /// Print the outcome as JSON instead of the tool's text.
        #[arg(long)]
        json: bool,
        /// Path of the file or directory on FROM_NODE.
        #[arg(value_name = "FROM")]
        from: String,
        /// Path the copy will have on TO_NODE: the file itself, or the directory that receives
        /// the tree.
        #[arg(value_name = "TO")]
        to: String,
    },
    /// Show the World's event log.
    Events {
        /// Only events after this sequence number.
        #[arg(long, default_value_t = 0)]
        since: u64,
        /// Maximum events to print.
        #[arg(long, default_value_t = 50)]
        limit: u32,
    },
}

/// Grant changes; publishing any grant replaces the "everyone may do everything" default.
#[derive(Debug, Subcommand)]
pub enum GrantsCommand {
    /// Allow a member (or `*` for every member) some verbs on some machines.
    Allow {
        /// Member name, or `*`.
        principal: String,
        /// Target machines: names, `tag:<tag>`, or `all`; repeatable.
        #[arg(long = "node", value_name = "SELECTOR", required = true)]
        nodes: Vec<String>,
        /// Verbs to allow, or `*`; repeatable.
        #[arg(long = "verb", value_name = "VERB", required = true)]
        verbs: Vec<String>,
        /// Expiry, RFC 3339 UTC.
        #[arg(long, value_name = "TIME")]
        expires: Option<String>,
        /// Replace the grant with this id instead of adding a new one.
        #[arg(long, value_name = "ID")]
        id: Option<String>,
    },
    /// Remove one grant by id.
    Remove {
        /// Grant id as shown by `fastctx world grants`.
        id: String,
    },
}

/// Commands for this machine's node.
#[derive(Debug, Subcommand)]
pub enum NodeCommand {
    /// Join a World with an invite from a member.
    Enroll {
        /// The invite string (`fxw2.…`).
        invite: String,
        /// This machine's name; defaults to the invite's suggestion.
        #[arg(long, value_name = "NAME")]
        name: Option<String>,
        /// Tags for selectors like `tag:office`; repeatable.
        #[arg(long = "tag", value_name = "TAG")]
        tags: Vec<String>,
        /// Network path for the hub link.
        #[arg(long, value_enum, default_value = "auto")]
        network: NetworkChoice,
        /// Physical interface to pin the direct path to.
        #[arg(long, value_name = "NAME")]
        interface: Option<String>,
        /// Write the enrollment but do not install or start the node service.
        #[arg(long)]
        no_service: bool,
    },
    /// Show this node's link, identity, and service state.
    Status {
        /// Print JSON instead of the summary.
        #[arg(long)]
        json: bool,
    },
    /// Leave the World: stop the service, tell the hub, delete the enrollment.
    Unenroll {
        /// Leave the service registration in place.
        #[arg(long)]
        keep_service: bool,
    },
    /// Register the node as a user-level service and start it.
    InstallService {
        /// Windows only: register the logon task for this user instead of the current one.
        #[arg(long, value_name = "USER")]
        user: Option<String>,
    },
    /// Stop and remove the service registration.
    UninstallService,
    /// Restart the node service (after an upgrade or a configuration change).
    Restart,
    /// Stop the node service until the next login or restart.
    Stop,
    /// Run the node in the foreground (what the service runs).
    Run {
        /// Explicit enrollment HOME used by an installed user service.
        #[arg(long, hide = true, value_name = "DIR")]
        home: Option<PathBuf>,
        /// Bind this node to the headless Windows task host's lifetime.
        #[arg(long, hide = true)]
        service_host: bool,
    },
}

fn hub_data_dir(explicit: Option<PathBuf>) -> Result<PathBuf, String> {
    if let Some(path) = explicit {
        return Ok(path);
    }
    let paths = ControlPaths::discover()?;
    Ok(paths.home.join(".fastctx-hub"))
}

pub async fn run_hub(command: HubCommand) -> Result<ExitCode, String> {
    match command {
        HubCommand::Serve {
            listen,
            data,
            cert,
            key,
            behind_proxy,
            reset_bootstrap,
        } => {
            let options = super::hub::HubOptions {
                listen,
                data: hub_data_dir(data)?,
                cert,
                key,
                behind_proxy,
                reset_bootstrap,
            };
            run_on_dedicated_runtime(super::hub::run(options)).await?;
            Ok(ExitCode::SUCCESS)
        }
        HubCommand::Status { data } => {
            let data = hub_data_dir(data)?;
            match super::hub::read_status(&data)? {
                None => {
                    println!(
                        "No hub is running from {} (no status file).",
                        crate::paths::display_path(&data)
                    );
                    Ok(ExitCode::FAILURE)
                }
                Some(status) => {
                    let live = super::hub::status_is_live(&status);
                    println!(
                        "{} hub {} (pid {}) on {}",
                        if live { "RUNNING" } else { "STALE" },
                        status.version,
                        status.pid,
                        status.listen
                    );
                    println!("World {}  hub key {}", status.world_id, status.hub_key);
                    println!("TLS: {}", status.tls);
                    println!(
                        "Binding: {}  started {}  status written {}",
                        status.binding, status.started_at, status.written_at
                    );
                    println!(
                        "Members: {} ({} online), open invites {}, data connections {}, events {}{}",
                        status.members.len(),
                        status
                            .members
                            .iter()
                            .filter(|member| member.state == "online")
                            .count(),
                        status.open_invites,
                        status.data_connections,
                        status.events,
                        if status.bootstrap_used {
                            ""
                        } else {
                            ", waiting for the first member"
                        }
                    );
                    for member in &status.members {
                        println!(
                            "  {:<32} {:<8} last seen {}  queued {}  fastctx {}",
                            member.name,
                            member.state,
                            if member.last_seen.is_empty() {
                                "never"
                            } else {
                                &member.last_seen
                            },
                            member.outbox,
                            if member.version.is_empty() {
                                "?"
                            } else {
                                &member.version
                            }
                        );
                    }
                    Ok(if live {
                        ExitCode::SUCCESS
                    } else {
                        ExitCode::FAILURE
                    })
                }
            }
        }
        HubCommand::Revoke { name, data } => {
            let data = hub_data_dir(data)?;
            println!("{}", super::hub::revoke_from_cli(&data, &name)?);
            Ok(ExitCode::SUCCESS)
        }
        HubCommand::Join { invite: _ } => Err(
            "fastctx hub join is not available in this version: the hub-as-member path ships with the web AI endpoint."
                .to_string(),
        ),
    }
}

pub async fn run_world(command: WorldCommand) -> Result<ExitCode, String> {
    let paths = ControlPaths::discover()?;
    match command {
        WorldCommand::Network => {
            use std::io::Read;
            let mut json = String::new();
            std::io::stdin()
                .read_to_string(&mut json)
                .map_err(|e| e.to_string())?;
            let request: super::resident::Request =
                serde_json::from_str(&json).map_err(|e| e.to_string())?;
            let response = super::node::admin::network_call(
                &crate::runtime::node_admin_endpoint(
                    &crate::session::SessionEnvironment::capture()?,
                )?,
                &request,
            )
            .await?;
            println!(
                "{}",
                serde_json::to_string(&response).map_err(|e| e.to_string())?
            );
            Ok(if response.ok {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            })
        }
        WorldCommand::Init {
            hub,
            bootstrap,
            name,
            tags,
            network,
            interface,
            no_service,
        } => {
            let options = super::enroll::EnrollOptions {
                name,
                tags,
                network: network.into(),
                interface,
            };
            let summary = super::enroll::bootstrap(&paths, &hub, &bootstrap, options).await?;
            print_enrollment(&summary);
            finish_enrollment(&paths, no_service)
        }
        WorldCommand::Invite { name, hours, hubs } => {
            let response = ask_daemon(&AdminRequest::Invite {
                name,
                ttl_hours: hours,
                hubs: (!hubs.is_empty()).then_some(hubs),
            })
            .await?;
            let invite = response
                .data
                .as_str()
                .ok_or_else(|| "The node service returned no invite.".to_string())?;
            println!("Run this on the new machine within {hours} hours (the invite works once):");
            println!();
            println!("    fastctx node enroll {invite}");
            println!();
            Ok(ExitCode::SUCCESS)
        }
        WorldCommand::Revoke { name } => {
            let response = ask_daemon(&AdminRequest::Revoke { name: name.clone() }).await?;
            let _ = response;
            println!("\"{name}\" was revoked.");
            Ok(ExitCode::SUCCESS)
        }
        WorldCommand::Grants {
            command:
                Some(GrantsCommand::Allow {
                    principal,
                    nodes,
                    verbs,
                    expires,
                    id,
                }),
        } => {
            if principal != "*" {
                super::validate_node_name(&principal)?;
            }
            for verb in &verbs {
                if verb != "*" && !super::grant::ALL_VERBS.contains(&verb.as_str()) {
                    return Err(format!(
                        "Unknown verb \"{verb}\". Verbs: {} or *.",
                        super::grant::ALL_VERBS.join(", ")
                    ));
                }
            }
            if let Some(expires) = &expires {
                super::parse_rfc3339(expires)?;
            }
            let response = ask_daemon(&AdminRequest::Grant {
                id,
                principal,
                nodes,
                verbs,
                expires,
                delete: false,
                expected_revision: None,
            })
            .await?;
            println!(
                "Published grant {} as revision {}. Members apply it as the hub relays it.",
                response
                    .data
                    .get("id")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("?"),
                response
                    .data
                    .get("revision")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0)
            );
            Ok(ExitCode::SUCCESS)
        }
        WorldCommand::Grants {
            command: Some(GrantsCommand::Remove { id }),
        } => {
            let response = ask_daemon(&AdminRequest::Grant {
                id: Some(id.clone()),
                principal: String::new(),
                nodes: Vec::new(),
                verbs: Vec::new(),
                expires: None,
                delete: true,
                expected_revision: None,
            })
            .await?;
            println!(
                "Removed grant {id} as revision {}.",
                response
                    .data
                    .get("revision")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0)
            );
            Ok(ExitCode::SUCCESS)
        }
        WorldCommand::Grants { command: None } => {
            let world_paths = super::WorldPaths::from_control(&paths);
            if !super::is_enrolled(&paths) {
                return Err(not_enrolled(&world_paths));
            }
            let grants = super::grant::GrantSet::load(&world_paths)?.unwrap_or_default();
            if grants.revision == 0 {
                println!(
                    "No grant has been published; the default applies: every member may use every verb on every member."
                );
            } else if grants.grants.is_empty() {
                println!(
                    "Revision {} (published by {} at {}) holds no grant; the default applies: every member may use every verb on every member.",
                    grants.revision, grants.published_by, grants.published_at
                );
            } else {
                println!(
                    "Grants in force (revision {}, published by {} at {}):",
                    grants.revision, grants.published_by, grants.published_at
                );
                for entry in &grants.grants {
                    println!(
                        "  {}: {} may {} on {}{}",
                        entry.id,
                        entry.grant.principal,
                        entry.grant.verbs.join(", "),
                        entry.grant.nodes.join(", "),
                        entry
                            .grant
                            .expires
                            .as_deref()
                            .map(|expires| format!(" until {expires}"))
                            .unwrap_or_default()
                    );
                }
            }
            let state = super::state::NodeState::load(&world_paths)?;
            if state.grant_floor > grants.revision {
                println!(
                    "Revision {} is known to exist but has not reached this node; calls from other members are refused as grant_stale until it does.",
                    state.grant_floor
                );
            }
            Ok(ExitCode::SUCCESS)
        }
        WorldCommand::Nodes {
            json,
            names,
            online,
            os,
            tags,
        } => {
            let response = ask_daemon(&AdminRequest::Nodes).await?;
            let mut nodes = super::node::admin::node_views_from(response.data)?;
            nodes.retain(|node| {
                (!online || node.state == "online")
                    && os
                        .as_deref()
                        .is_none_or(|os| node.os.eq_ignore_ascii_case(os))
                    && tags.iter().all(|tag| node.tags.contains(tag))
            });
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&nodes).unwrap_or_default()
                );
            } else if names {
                for node in &nodes {
                    println!("{}", node.name);
                }
            } else {
                print_nodes(&nodes);
            }
            Ok(ExitCode::SUCCESS)
        }
        WorldCommand::Run {
            node,
            cwd,
            timeout,
            json,
            command,
        } => {
            let timeout = parse_duration(&timeout)?;
            let me = match self_name().await {
                Ok(me) => me,
                Err(error) => return Ok(world_failure(&node, &error, json)),
            };
            let cwd = match cwd {
                Some(cwd) => Some(cwd),
                None if node == me => std::env::current_dir()
                    .ok()
                    .map(|directory| crate::paths::display_path(&directory)),
                None => None,
            };
            let args = serde_json::json!({
                "command": command.join(" "),
                "cwd": cwd,
                "timeout_ms": timeout.as_millis() as u64,
            });
            let outcome = match call_daemon("run", node.clone(), args, timeout).await {
                Ok(outcome) => outcome,
                Err(error) => return Ok(world_failure(&node, &error, json)),
            };
            let code = exit_code_of(&outcome);
            if json {
                print_outcome_json(&outcome, code);
            } else {
                print_outcome_text(&outcome, code);
            }
            Ok(ExitCode::from(code))
        }
        WorldCommand::Copy {
            from_node,
            to_node,
            timeout,
            json,
            from,
            to,
        } => {
            let timeout = parse_duration(&timeout)?;
            if from_node.is_none() && to_node.is_none() {
                return Err(
                    "copy moves a file or directory between two machines; give --from-node or --to-node."
                        .to_string(),
                );
            }
            let me = match self_name().await {
                Ok(me) => me,
                Err(error) => {
                    let named = from_node
                        .as_deref()
                        .or(to_node.as_deref())
                        .unwrap_or_default();
                    return Ok(world_failure(named, &error, json));
                }
            };
            let from_node = from_node.unwrap_or_else(|| me.clone());
            let to_node = to_node.unwrap_or_else(|| me.clone());
            if from_node == to_node {
                return Err(
                    "--from-node and --to-node name the same machine; copy works between two machines."
                        .to_string(),
                );
            }
            let args = serde_json::to_value(super::copy::CopyArgs {
                from,
                to,
                to_node,
                caller: me,
                timeout_ms: Some(timeout.as_millis() as u64),
            })
            .map_err(|error| error.to_string())?;
            let outcome = match call_daemon("copy", from_node.clone(), args, timeout).await {
                Ok(outcome) => outcome,
                Err(error) => return Ok(world_failure(&from_node, &error, json)),
            };
            let code = match &outcome.response {
                Some(response) if response.is_error => 1,
                Some(_) => {
                    if outcome
                        .facts
                        .get("failed")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(0)
                        > 0
                    {
                        1
                    } else {
                        0
                    }
                }
                None => WORLD_FAILURE_EXIT,
            };
            if json {
                print_outcome_json(&outcome, code);
            } else {
                print_outcome_text(&outcome, code);
            }
            Ok(ExitCode::from(code))
        }
        WorldCommand::Events { since, limit } => {
            let response = ask_daemon(&AdminRequest::Events { since, limit }).await?;
            let events: super::messages::EventsResult = serde_json::from_value(response.data)
                .map_err(|error| format!("Unreadable event log: {error}"))?;
            for event in &events.events {
                let facts = event
                    .facts
                    .iter()
                    .map(|(key, value)| {
                        format!(
                            "{key}={}",
                            value
                                .as_str()
                                .map(str::to_string)
                                .unwrap_or_else(|| value.to_string())
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                println!(
                    "#{:<6} {}  {:<14} {:<24} {facts}",
                    event.seq, event.at, event.kind, event.subject
                );
            }
            if events.events.is_empty() {
                println!("No events after #{since}. Newest is #{}.", events.latest);
            } else {
                println!("Newest event is #{}.", events.latest);
            }
            Ok(ExitCode::SUCCESS)
        }
    }
}

pub async fn run_node(command: NodeCommand) -> Result<ExitCode, String> {
    let paths = match &command {
        NodeCommand::Run {
            home: Some(home), ..
        } => {
            if !home.is_absolute() {
                return Err("The node service HOME must be an absolute directory.".to_string());
            }
            ControlPaths::for_home(home)
        }
        _ => ControlPaths::discover()?,
    };
    let world_paths = super::WorldPaths::from_control(&paths);
    match command {
        NodeCommand::Enroll {
            invite,
            name,
            tags,
            network,
            interface,
            no_service,
        } => {
            let options = super::enroll::EnrollOptions {
                name: name.unwrap_or_default(),
                tags,
                network: network.into(),
                interface,
            };
            let summary = super::enroll::enroll(&paths, &invite, options).await?;
            print_enrollment(&summary);
            finish_enrollment(&paths, no_service)
        }
        NodeCommand::Status { json } => {
            if !super::is_enrolled(&paths) {
                println!("This machine is not enrolled in a World.");
                return Ok(ExitCode::FAILURE);
            }
            let live = match daemon(&AdminRequest::Status).await? {
                Some(response) if response.ok => Some(response.data),
                _ => None,
            };
            let (status, source) = match live {
                Some(data) => (
                    serde_json::from_value::<super::node::admin::NodeStatus>(data)
                        .map_err(|error| format!("Unreadable node status: {error}"))?,
                    "live",
                ),
                None => match super::node::read_status(&world_paths)? {
                    Some(status) => (status, "stale"),
                    None => {
                        println!(
                            "The FastCtx node service is not running on this machine ({}).",
                            if super::node::service::is_installed() {
                                "the service is installed but not running; run 'fastctx node restart'"
                            } else {
                                "no service is installed; run 'fastctx node install-service'"
                            }
                        );
                        return Ok(ExitCode::FAILURE);
                    }
                },
            };
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&status).unwrap_or_default()
                );
                return Ok(ExitCode::SUCCESS);
            }
            print_node_status(&status, source);
            Ok(if source == "live" {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            })
        }
        NodeCommand::Unenroll { keep_service } => {
            if !super::is_enrolled(&paths) {
                return Err(not_enrolled(&world_paths));
            }
            match daemon(&AdminRequest::Leave).await? {
                Some(response) if response.ok => {
                    println!("Told the hub this member is leaving.");
                    tokio::time::sleep(std::time::Duration::from_millis(800)).await;
                }
                Some(response) => println!(
                    "Could not tell the hub ({}); leaving locally.",
                    response.error.unwrap_or_default()
                ),
                None => println!(
                    "The node service is not running; leaving locally (the hub learns of it when a member revokes this name)."
                ),
            }
            if !keep_service {
                if let Ok(message) = super::node::service::stop(&paths) {
                    println!("{message}")
                }
                match super::node::service::uninstall(&paths) {
                    Ok(message) => println!("{message}"),
                    Err(error) => println!("{error}"),
                }
            } else {
                let _ = super::node::service::stop(&paths);
            }
            super::remove_config(&world_paths)?;
            if let Err(error) = std::fs::remove_dir_all(&world_paths.dir)
                && error.kind() != std::io::ErrorKind::NotFound
            {
                return Err(format!(
                    "Removed world.toml but cannot delete {}: {error}",
                    crate::paths::display_path(&world_paths.dir)
                ));
            }
            println!(
                "This machine left the World. Restart your agent sessions to return to the local tool surface, and run 'fastctx apply' to drop the World section from agent guidance."
            );
            Ok(ExitCode::SUCCESS)
        }
        NodeCommand::InstallService { user } => {
            if !super::is_enrolled(&paths) {
                return Err(not_enrolled(&world_paths));
            }
            println!(
                "{}",
                super::node::service::install(&paths, user.as_deref())?
            );
            Ok(ExitCode::SUCCESS)
        }
        NodeCommand::UninstallService => {
            println!("{}", super::node::service::uninstall(&paths)?);
            Ok(ExitCode::SUCCESS)
        }
        NodeCommand::Restart => {
            println!("{}", super::node::service::restart(&paths)?);
            Ok(ExitCode::SUCCESS)
        }
        NodeCommand::Stop => {
            println!("{}", super::node::service::stop(&paths)?);
            Ok(ExitCode::SUCCESS)
        }
        NodeCommand::Run { service_host, .. } => {
            run_on_dedicated_runtime(super::node::run_daemon(paths, service_host)).await?;
            Ok(ExitCode::SUCCESS)
        }
    }
}

fn not_enrolled(world_paths: &super::WorldPaths) -> String {
    format!(
        "This machine is not enrolled in a World ({} does not exist). Run 'fastctx node enroll <invite>' or 'fastctx world init'.",
        crate::paths::display_path(&world_paths.config)
    )
}

fn print_enrollment(summary: &super::enroll::EnrollSummary) {
    println!(
        "Enrolled as \"{}\" in World {} via {} ({}).",
        summary.name,
        summary.world_id,
        summary.hub.join(", "),
        summary.path
    );
    println!("Hub key {}  TLS {}", summary.hub_key, summary.tls.as_str());
}

fn finish_enrollment(paths: &ControlPaths, no_service: bool) -> Result<ExitCode, String> {
    if no_service {
        println!(
            "The node service was not installed; run 'fastctx node install-service' or 'fastctx node run'."
        );
        return Ok(ExitCode::SUCCESS);
    }
    match super::node::service::install(paths, None) {
        Ok(message) => {
            println!("{message}");
            println!(
                "Run 'fastctx node status' to watch the link, then 'fastctx apply' on agent machines to publish the World tools."
            );
            Ok(ExitCode::SUCCESS)
        }
        Err(error) => {
            println!("{error}");
            println!(
                "The enrollment is written; start the node with 'fastctx node run' or fix the service and run 'fastctx node install-service'."
            );
            Ok(ExitCode::FAILURE)
        }
    }
}

fn print_nodes(nodes: &[super::client::NodeView]) {
    if nodes.is_empty() {
        println!("No members are known yet.");
        return;
    }
    let online = nodes.iter().filter(|node| node.state == "online").count();
    println!("{} nodes, {online} online", nodes.len());
    for node in nodes {
        let facts = node
            .inventory
            .as_ref()
            .map(|inventory| {
                let mut parts = vec![
                    format!("cpus {}", inventory.cpus),
                    format!("mem {}G", inventory.memory_gb),
                ];
                if !inventory.gpus.is_empty() {
                    parts.push(format!(
                        "gpu {}",
                        inventory
                            .gpus
                            .iter()
                            .map(|gpu| format!("{} {}G", gpu.model, gpu.memory_gb))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                }
                if inventory.wsl2 == Some(true) {
                    parts.push("wsl2".to_string());
                }
                if !inventory.home.is_empty() {
                    parts.push(format!("home {}", inventory.home));
                }
                if !inventory.user.is_empty() {
                    parts.push(format!("user {}", inventory.user));
                }
                parts.join("  ")
            })
            .unwrap_or_default();
        let tags = if node.tags.is_empty() {
            String::new()
        } else {
            format!("tags {}", node.tags.join(","))
        };
        let link = match (node.network.as_deref(), node.hub_rtt_ms) {
            (Some(network), Some(rtt)) => format!("{network} {rtt}ms"),
            (Some(network), None) => network.to_string(),
            _ => String::new(),
        };
        let tail = [
            if node.state == "online" {
                facts
            } else {
                format!("last seen {}", node.last_seen)
            },
            tags,
            link,
            node.effective_note()
                .map(|note| format!("note {}", super::notes::quoted(note)))
                .unwrap_or_default(),
        ]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("  ");
        println!(
            "{:<16} {:<16} {:<8} {tail}{}",
            node.name,
            format!("{}/{}", node.os, node.arch),
            node.state,
            if node.is_self { "  (this machine)" } else { "" }
        );
    }
}

fn print_node_status(status: &super::node::admin::NodeStatus, source: &str) {
    let link = &status.link;
    let state = match &link.state {
        super::client::LinkState::Starting => "starting".to_string(),
        super::client::LinkState::Connecting { attempt } => {
            format!("connecting (attempt {attempt})")
        }
        super::client::LinkState::Connected => "connected".to_string(),
        super::client::LinkState::Reconnecting {
            attempt,
            next_attempt_at,
        } => {
            format!("reconnecting (attempt {attempt}, next at {next_attempt_at})")
        }
        super::client::LinkState::Stopped { reason, until } => match until {
            Some(until) => format!("paused until {until}: {reason}"),
            None => format!("stopped: {reason}"),
        },
    };
    println!(
        "{} node \"{}\" (pid {}, fastctx {}) in World {}{}",
        if source == "live" {
            "RUNNING"
        } else {
            "STALE STATUS FROM"
        },
        status.name,
        status.pid,
        status.version,
        status.world_id,
        if source == "live" {
            String::new()
        } else {
            format!(", written {}", status.written_at)
        }
    );
    println!("Hub: {} (key {})", status.hub.join(", "), status.hub_key);
    println!("Link: {state}");
    if let Some(path) = &link.path {
        println!("Network: {path}");
    }
    if let (false, Some(interface)) = (link.tunnels.is_empty(), link.interface.as_deref()) {
        println!(
            "A TUN adapter is active ({}); the hub link is pinned to {interface} and bypasses it.",
            link.tunnels.join(", ")
        );
    }
    println!(
        "TLS {}  outbox {}  replaced {}",
        link.tls.as_str(),
        link.outbox_depth,
        link.replaced_count
    );
    println!(
        "Heartbeat sent {}  ack {}  rtt {}",
        link.last_heartbeat_at.as_deref().unwrap_or("never"),
        link.last_ack_at.as_deref().unwrap_or("never"),
        link.rtt_ms
            .map(|rtt| format!("{rtt} ms"))
            .unwrap_or_else(|| "?".to_string())
    );
    println!(
        "Members: {} known, {} online; grant revision {}{}; running calls {}; control center {}",
        status.members,
        status.members_online,
        status.grant_revision,
        if status.grant_stale {
            " (a newer revision is pending; remote calls refused as grant_stale)"
        } else {
            ""
        },
        status.running_calls,
        if status.engine_hosted {
            "hosted by this node"
        } else {
            "not hosted"
        }
    );
    if let Some(error) = &link.last_error {
        println!("Last error: {error}");
    }
}

/// Exit code of `fastctx world run` / `copy` when the World layer, not the command, failed:
/// the machine was offline, forbidden, silent, or this machine is not enrolled. The same
/// number `ssh` uses for its own failures, so a script can tell "failed there" from "never
/// got there".
const WORLD_FAILURE_EXIT: u8 = 255;
/// Exit code when the remote command was stopped at the timeout, as `timeout(1)` reports it.
const TIMEOUT_EXIT: u8 = 124;

/// Sends one admin request to the running daemon; `Ok(None)` when it is not running.
async fn daemon(request: &AdminRequest) -> Result<Option<AdminResponse>, String> {
    daemon_within(request, super::node::admin::ADMIN_TIMEOUT).await
}

async fn daemon_within(
    request: &AdminRequest,
    timeout: std::time::Duration,
) -> Result<Option<AdminResponse>, String> {
    let paths = ControlPaths::discover()?;
    if !super::is_enrolled(&paths) {
        return Err(format!(
            "not_enrolled: this machine is not enrolled in a World ({} does not exist). Run 'fastctx node enroll <invite>' or 'fastctx world init'.",
            crate::paths::display_path(&paths.fastctx_dir.join("world.toml"))
        ));
    }
    let environment = crate::session::SessionEnvironment::capture()?;
    let endpoint = crate::runtime::node_admin_endpoint(&environment)?;
    super::node::admin::call(&endpoint, request, timeout).await
}

/// Like `daemon`, but the daemon must be running and must succeed.
async fn ask_daemon(request: &AdminRequest) -> Result<AdminResponse, String> {
    ask_daemon_within(request, super::node::admin::ADMIN_TIMEOUT).await
}

async fn ask_daemon_within(
    request: &AdminRequest,
    timeout: std::time::Duration,
) -> Result<AdminResponse, String> {
    match daemon_within(request, timeout).await? {
        Some(response) if response.ok => Ok(response),
        Some(response) => Err(response
            .error
            .unwrap_or_else(|| "The node service reported a failure.".to_string())),
        None => Err(
            "The FastCtx node service is not running on this machine; run 'fastctx node status'."
                .to_string(),
        ),
    }
}

/// This machine's name in the World, from the running daemon.
async fn self_name() -> Result<String, String> {
    let response = ask_daemon(&AdminRequest::Status).await?;
    let status: super::node::admin::NodeStatus = serde_json::from_value(response.data)
        .map_err(|error| format!("Unreadable node status: {error}"))?;
    Ok(status.name)
}

/// Runs one verb on one machine through the daemon and returns the outcome; a World-layer
/// failure is an `Err` the caller turns into exit 255.
async fn call_daemon(
    verb: &str,
    node: String,
    args: serde_json::Value,
    timeout: std::time::Duration,
) -> Result<super::node::admin::CallOutcome, String> {
    let request = AdminRequest::Call {
        verb: verb.to_string(),
        node,
        args,
        timeout_ms: timeout.as_millis() as u64,
    };
    let response = ask_daemon_within(&request, timeout + super::node::admin::ADMIN_TIMEOUT).await?;
    serde_json::from_value(response.data)
        .map_err(|error| format!("Unreadable answer from the node service: {error}"))
}

/// Parses `90`, `90s`, `10m`, `2h`, or `1d`; at least a second, at most a day.
fn parse_duration(text: &str) -> Result<std::time::Duration, String> {
    let text = text.trim();
    let (digits, unit) = match text.find(|character: char| !character.is_ascii_digit()) {
        Some(split) => text.split_at(split),
        None => (text, "s"),
    };
    let amount = digits
        .parse::<u64>()
        .map_err(|_| format!("Invalid duration \"{text}\": use a number with s, m, h, or d."))?;
    let seconds = match unit.trim() {
        "s" | "sec" | "secs" => amount,
        "m" | "min" | "mins" => amount * 60,
        "h" | "hr" | "hrs" => amount * 3600,
        "d" => amount * 86_400,
        _ => {
            return Err(format!(
                "Invalid duration \"{text}\": use a number with s, m, h, or d."
            ));
        }
    };
    let ceiling = crate::shell::MAX_SCRIPT_CALL_MS / 1000;
    if seconds == 0 || seconds > ceiling {
        return Err(format!("Invalid duration \"{text}\": between 1s and 24h."));
    }
    Ok(std::time::Duration::from_secs(seconds))
}

/// The exit code a `run` outcome maps to: the command's own, 124 at the timeout, 255 when the
/// World layer or the tool itself failed.
fn exit_code_of(outcome: &super::node::admin::CallOutcome) -> u8 {
    match &outcome.response {
        Some(response) => match outcome
            .facts
            .get("exit_code")
            .and_then(serde_json::Value::as_i64)
        {
            Some(code) => code.clamp(0, 255) as u8,
            None if response.is_error => WORLD_FAILURE_EXIT,
            None => TIMEOUT_EXIT,
        },
        None => WORLD_FAILURE_EXIT,
    }
}

fn outcome_text(outcome: &super::node::admin::CallOutcome) -> String {
    outcome
        .response
        .as_ref()
        .map(|response| {
            response
                .content
                .iter()
                .filter_map(|block| match block {
                    super::messages::WireContent::Text { text } => Some(text.as_str()),
                    super::messages::WireContent::Image { .. } => None,
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

/// Prints the tool's text to stdout, or the World layer's sentence to stderr.
fn print_outcome_text(outcome: &super::node::admin::CallOutcome, code: u8) {
    match &outcome.response {
        Some(response) => {
            let text = outcome_text(outcome);
            if response.is_error && code == WORLD_FAILURE_EXIT {
                eprintln!("{}: {text}", outcome.node);
            } else {
                println!("{text}");
            }
        }
        None => eprintln!(
            "{}: {}{}",
            outcome.node,
            outcome.status,
            outcome
                .message
                .as_deref()
                .map(|message| format!(" ({message})"))
                .unwrap_or_default()
        ),
    }
}

/// Prints the outcome as one JSON object; `exit_code` is the number the process exits with.
fn print_outcome_json(outcome: &super::node::admin::CallOutcome, code: u8) {
    let value = serde_json::json!({
        "node": outcome.node,
        "status": outcome.status,
        "message": outcome.message,
        "exit_code": code,
        "facts": outcome.facts,
        "text": outcome_text(outcome),
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&value).unwrap_or_default()
    );
}

/// A failure before any machine answered: this machine is not enrolled, its node service is
/// not running, or the named machine is unknown. Printed the way an unreachable machine's
/// outcome is, and exiting with the code scripts read as "never got there".
fn world_failure(node: &str, error: &str, json: bool) -> ExitCode {
    if json {
        let value = serde_json::json!({
            "node": node,
            "status": "failed",
            "message": error,
            "exit_code": WORLD_FAILURE_EXIT,
            "facts": {},
            "text": "",
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&value).unwrap_or_default()
        );
    } else {
        eprintln!("fastctx: {error}");
    }
    ExitCode::from(WORLD_FAILURE_EXIT)
}

/// Runs a long-lived server on its own multi-threaded runtime, outside the CLI's
/// single-threaded one, so store writes and TLS work never stall frame handling.
async fn run_on_dedicated_runtime<F>(future: F) -> Result<(), String>
where
    F: std::future::Future<Output = Result<(), String>> + Send + 'static,
{
    tokio::task::spawn_blocking(move || {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .map_err(|error| format!("Cannot start the server runtime: {error}"))?;
        runtime.block_on(future)
    })
    .await
    .map_err(|error| format!("The server runtime stopped unexpectedly: {error}"))?
}
