//! The commands that make machines into one World: joining, inviting,
//! revoking, leaving, and seeing the machines (`world-network.md`).

use clap::{Args, Subcommand};
use fastctx_types::world::{LinkState, NodesReply, RelaySetting, Upstream};
use fastctx_types::{Call, MachineName, Reply};
use jiff::{SignedDuration, Timestamp};

#[derive(Args)]
pub struct JoinArgs {
    /// The code: a hub's founding code, or an invitation a member made.
    code: String,
    /// The name to join under; this machine's own name when not given.
    #[arg(long)]
    name: Option<MachineName>,
}

#[derive(Args)]
pub struct InviteArgs {
    /// The name the joining machine must take.
    #[arg(long)]
    name: Option<MachineName>,
    /// How many hours the invitation stays valid.
    #[arg(long, default_value_t = 1)]
    hours: u32,
    /// What it may do; everything when nothing is limited.
    #[command(flatten)]
    scope: crate::scope_args::ScopeArgs,
    /// Print, instead of the code, the one line a Linux server runs to
    /// download fastctx from the hub, check it, install it and join.
    #[arg(long)]
    server: bool,
}

#[derive(Args)]
pub struct GrantArgs {
    /// The member whose scope this sets.
    member: MachineName,
    /// It holds management rights from now on, and may do everything.
    #[arg(long)]
    manage: bool,
    /// What it may do; everything when nothing is limited.
    #[command(flatten)]
    scope: crate::scope_args::ScopeArgs,
}

#[derive(Args)]
pub struct RouteArgs {
    /// The member whose route this sets.
    member: MachineName,
    /// A relay it connects through; give it once for each, in the order it
    /// tries them. The hub comes after them; with none, it connects straight
    /// to the hub.
    #[arg(long = "via")]
    via: Vec<MachineName>,
}

#[derive(Args)]
pub struct RelayArgs {
    /// The machine that relays; this one when not given.
    #[arg(long)]
    machine: Option<MachineName>,
    /// An address the machines below it dial, as IP:PORT; give it once for
    /// each.
    #[arg(long = "public", required_unless_present = "off")]
    public: Vec<std::net::SocketAddr>,
    /// Where it listens; all addresses, on the first public port, when not
    /// given.
    #[arg(long)]
    listen: Option<std::net::SocketAddr>,
    /// Stop relaying.
    #[arg(long, conflicts_with_all = ["public", "listen"])]
    off: bool,
}

#[derive(Args)]
pub struct RevokeArgs {
    /// The machine whose key no machine should trust any more.
    machine: MachineName,
    /// Why, kept with the revocation.
    #[arg(long, default_value = "revoked")]
    reason: String,
}

#[derive(Subcommand)]
pub enum HubCommand {
    /// Create a hub on this machine and print the code that founds its World.
    Init {
        /// An address machines reach the hub at, as IP:PORT; give it once for
        /// each.
        #[arg(long = "public", required = true)]
        public: Vec<std::net::SocketAddr>,
        /// Where the hub listens; all addresses, on the first public port,
        /// when not given.
        #[arg(long)]
        listen: Option<std::net::SocketAddr>,
        /// Where the hub keeps its files.
        #[arg(long, value_parser = crate::paths::parser())]
        dir: Option<std::path::PathBuf>,
    },
    /// Run the hub in the foreground.
    Serve {
        #[arg(long, value_parser = crate::paths::parser())]
        dir: Option<std::path::PathBuf>,
    },
    /// Print the founding code again, until a machine has used it.
    Code {
        #[arg(long, value_parser = crate::paths::parser())]
        dir: Option<std::path::PathBuf>,
    },
}

pub fn join(args: JoinArgs) -> anyhow::Result<()> {
    match call(Call::Join { code: args.code, name: args.name })? {
        Reply::Joined { world, name, .. } => println!("joined the World {world} as {name}"),
        other => anyhow::bail!("the node answered something else: {other:?}"),
    }
    crate::keep::keep_connected()
}

pub fn invite(args: InviteArgs) -> anyhow::Result<()> {
    let expires = Timestamp::now() + SignedDuration::from_hours(i64::from(args.hours));
    let until = expires.to_zoned(jiff::tz::TimeZone::system()).strftime("%Y-%m-%d %H:%M %Z");
    if args.server {
        match call(Call::InviteServer { name: args.name.clone(), expires, scope: args.scope.scope() })? {
            Reply::ServerInvitation { command } => {
                println!("{command}");
                eprintln!("Paste this line into the server's terminal. It works once, until {until}.");
            }
            other => anyhow::bail!("the node answered something else: {other:?}"),
        }
        return Ok(());
    }
    match call(Call::Invite { name: args.name.clone(), expires, device: false, scope: args.scope.scope() })? {
        Reply::Code { code } => {
            println!("{code}");
            let named = args.name.map(|name| format!(" as {name}")).unwrap_or_default();
            eprintln!(
                "On the machine to invite, run `fastctx join <code>`{named}. The code works once, until {until}."
            );
        }
        other => anyhow::bail!("the node answered something else: {other:?}"),
    }
    Ok(())
}

pub fn grant(args: GrantArgs) -> anyhow::Result<()> {
    match call(Call::Grant { member: args.member.clone(), scope: args.scope.scope_or_all(), manage: args.manage })? {
        Reply::Done => println!("{} has its new scope; every machine of the World holds it to that", args.member),
        other => anyhow::bail!("the node answered something else: {other:?}"),
    }
    Ok(())
}

pub fn route(args: RouteArgs) -> anyhow::Result<()> {
    let via: Vec<Upstream> = args.via.iter().cloned().map(|machine| Upstream::Relay { machine }).collect();
    let said = if via.is_empty() {
        "straight to the hub".to_string()
    } else {
        let relays: Vec<String> = args.via.iter().map(ToString::to_string).collect();
        format!("through {}, then the hub", relays.join(", then "))
    };
    let via = if via.is_empty() { vec![Upstream::Hub] } else { via };
    match call(Call::Route { member: args.member.clone(), via })? {
        Reply::Done => println!("{} connects {said} from now on", args.member),
        other => anyhow::bail!("the node answered something else: {other:?}"),
    }
    Ok(())
}

pub fn relay(args: RelayArgs) -> anyhow::Result<()> {
    let machine = match args.machine {
        Some(machine) => machine,
        None => match call(Call::Nodes)? {
            Reply::Nodes(reply) => reply.machine,
            other => anyhow::bail!("the node answered something else: {other:?}"),
        },
    };
    let relay = if args.off {
        None
    } else {
        let first = args.public.first().expect("clap requires a public address");
        let listen = args.listen.unwrap_or_else(|| {
            let any = if first.is_ipv4() {
                std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED)
            } else {
                std::net::IpAddr::V6(std::net::Ipv6Addr::UNSPECIFIED)
            };
            std::net::SocketAddr::new(any, first.port())
        });
        Some(RelaySetting { listen, public: args.public })
    };
    match call(Call::SetRelay { machine: machine.clone(), relay })? {
        Reply::Relaying { relays_at } if relays_at.is_empty() => println!("{machine} relays for no machine now"),
        Reply::Relaying { relays_at } => println!(
            "{machine} relays at {}; `fastctx route <member> --via {machine}` puts a machine below it",
            relays_at.join(", ")
        ),
        other => anyhow::bail!("the node answered something else: {other:?}"),
    }
    Ok(())
}

pub fn revoke(args: RevokeArgs) -> anyhow::Result<()> {
    match call(Call::Revoke { name: args.machine.clone(), reason: args.reason })? {
        Reply::Done => println!("revoked {}: no machine of the World trusts its key any more", args.machine),
        other => anyhow::bail!("the node answered something else: {other:?}"),
    }
    Ok(())
}

pub fn leave() -> anyhow::Result<()> {
    match call(Call::Leave)? {
        Reply::Done => println!("left the World; this machine is a World of one again"),
        other => anyhow::bail!("the node answered something else: {other:?}"),
    }
    Ok(())
}

pub fn nodes() -> anyhow::Result<()> {
    match call(Call::Nodes)? {
        Reply::Nodes(reply) => print!("{}", render_nodes(&reply, Timestamp::now())),
        other => anyhow::bail!("the node answered something else: {other:?}"),
    }
    Ok(())
}

pub fn hub(command: HubCommand) -> anyhow::Result<()> {
    let root = fastctx_platform::fastctx_root()?;
    match command {
        HubCommand::Init { public, listen, dir } => {
            let dir = dir.unwrap_or_else(|| fastctx_hub::default_dir(&root));
            let listen = listen.unwrap_or_else(|| std::net::SocketAddr::from(([0, 0, 0, 0], public[0].port())));
            let code = fastctx_hub::init(&dir, public, listen)?;
            println!("{}", code.to_text());
            eprintln!(
                "A hub for the World {} is in {}. Start it with `fastctx hub serve`; then run `fastctx join <code>` on the first machine.",
                code.world(),
                dir.display()
            );
        }
        HubCommand::Serve { dir } => {
            let dir = dir.unwrap_or_else(|| fastctx_hub::default_dir(&root));
            tokio::runtime::Builder::new_multi_thread().enable_all().build()?.block_on(fastctx_hub::serve(&dir))?;
        }
        HubCommand::Code { dir } => {
            let dir = dir.unwrap_or_else(|| fastctx_hub::default_dir(&root));
            println!("{}", fastctx_hub::founding_code(&dir)?.to_text());
        }
    }
    Ok(())
}

fn call(call: Call) -> anyhow::Result<Reply> {
    Ok(crate::ask::call(call)?.0)
}

fn render_nodes(reply: &NodesReply, now: Timestamp) -> String {
    let link = match (&reply.world, &reply.link) {
        (_, LinkState::Alone) => "a World of one machine".to_string(),
        (Some(_), LinkState::Online { since, hub, relay: None, route }) => {
            format!("connected to the hub at {hub}, {route}, for {}", age(*since, now))
        }
        (Some(_), LinkState::Online { since, hub, relay: Some(relay), route }) => {
            format!("connected through the relay {relay} at {hub}, {route}, for {}", age(*since, now))
        }
        (Some(_), LinkState::Connecting { last_error: None, .. }) => "connecting to the hub".to_string(),
        (Some(_), LinkState::Connecting { since, last_error: Some(error) }) => {
            format!("not connected to the hub for {}: {error}", age(*since, now))
        }
        (Some(_), LinkState::Stopped { reason }) => format!("the hub refused this machine: {reason}"),
        (None, _) => "in a World".to_string(),
    };
    let mut text = format!("=== nodes: {} ({link}) ===\n", reply.machine);
    let width = reply.machines.iter().map(|node| node.name.len()).max().unwrap_or(0);
    for node in &reply.machines {
        let state = match node.online {
            _ if node.name == reply.machine => "this machine".to_string(),
            _ if node.revoked.is_some() => {
                let revoked = node.revoked.as_ref().expect("checked");
                format!("revoked by {} {} ago: {}", revoked.by, age(revoked.at, now), revoked.reason)
            }
            Some(true) => match node.since {
                Some(since) => format!("online for {}", age(since, now)),
                None => "online".to_string(),
            },
            Some(false) => match node.since {
                Some(since) => format!("offline for {}", age(since, now)),
                None => "offline".to_string(),
            },
            None => "not known".to_string(),
        };
        let facts = match &node.facts {
            Ok(facts) => {
                let shell = if facts.shell.is_empty() { String::new() } else { format!(" · {}", facts.shell) };
                format!(
                    "{} {}{shell} · home {} · temp {} · file names {} · fastctx {}",
                    facts.os,
                    facts.arch,
                    facts.home,
                    facts.temp,
                    if facts.case_sensitive { "case-sensitive" } else { "ignore case" },
                    facts.version
                )
            }
            Err(_) if node.online == Some(false) || node.revoked.is_some() => String::new(),
            Err(error) => format!("not known: {error}"),
        };
        let place = node.place.as_ref().map(place).unwrap_or_default();
        let line = format!("{:width$}  {state}{place}", node.name);
        if facts.is_empty() {
            text.push_str(&format!("{line}\n"));
        } else {
            text.push_str(&format!("{line}  {facts}\n"));
        }
        if let Ok(facts) = &node.facts
            && let Some(warning) = facts.node_lifetime.as_ref().and_then(|lifetime| lifetime.warning())
        {
            text.push_str(&format!("  {warning}\n"));
        }
    }
    text
}

/// Where a machine hangs in the tree, when that is more than straight below
/// the hub.
fn place(place: &fastctx_types::world::Place) -> String {
    let relays: Vec<String> = place
        .via
        .iter()
        .filter_map(|upstream| match upstream {
            Upstream::Relay { machine } => Some(machine.to_string()),
            Upstream::Hub => None,
        })
        .collect();
    let mut said = String::new();
    if !relays.is_empty() {
        said.push_str(&format!(" · through {}", relays.join(", then ")));
    }
    if !place.relays_at.is_empty() {
        said.push_str(&format!(" · relays at {}", place.relays_at.join(", ")));
    }
    said
}

/// How long ago a moment was, written as `w` writes it: the machines of a
/// World keep different clocks and zones, and a reader may know none of them.
fn age(at: Timestamp, now: Timestamp) -> String {
    fastctx_hands::told::duration(now.as_second().saturating_sub(at.as_second()).max(0) as u64)
}
