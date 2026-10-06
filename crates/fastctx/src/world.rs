//! The commands that make machines into one World: joining, inviting,
//! revoking, leaving, and seeing the machines (`world-network.md`).

use clap::{Args, Subcommand};
use fastctx_types::world::{LinkState, NodesReply};
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
        #[arg(long)]
        dir: Option<std::path::PathBuf>,
    },
    /// Run the hub in the foreground.
    Serve {
        #[arg(long)]
        dir: Option<std::path::PathBuf>,
    },
    /// Print the founding code again, until a machine has used it.
    Code {
        #[arg(long)]
        dir: Option<std::path::PathBuf>,
    },
}

pub fn join(args: JoinArgs) -> anyhow::Result<()> {
    match call(Call::Join { code: args.code, name: args.name })? {
        Reply::Joined { world, name } => println!("joined the World {world} as {name}"),
        other => anyhow::bail!("the node answered something else: {other:?}"),
    }
    crate::keep::keep_connected()
}

pub fn invite(args: InviteArgs) -> anyhow::Result<()> {
    let expires = Timestamp::now() + SignedDuration::from_hours(i64::from(args.hours));
    match call(Call::Invite { name: args.name.clone(), expires, device: false, scope: args.scope.scope() })? {
        Reply::Code { code } => {
            println!("{code}");
            let named = args.name.map(|name| format!(" as {name}")).unwrap_or_default();
            let until = expires.to_zoned(jiff::tz::TimeZone::system()).strftime("%Y-%m-%d %H:%M %Z");
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
        (Some(world), LinkState::Online { since, hub, route }) => {
            format!("in {world}; connected to the hub at {hub}, {route}, since {}", clock(*since, now))
        }
        (Some(world), LinkState::Connecting { last_error: None, .. }) => format!("in {world}; connecting to the hub"),
        (Some(world), LinkState::Connecting { since, last_error: Some(error) }) => {
            format!("in {world}; not connected to the hub since {}: {error}", clock(*since, now))
        }
        (Some(world), LinkState::Stopped { reason }) => {
            format!("in {world}; the hub refused this machine: {reason}")
        }
        (None, _) => "in a World".to_string(),
    };
    let mut text = format!("=== nodes: {} ({link}) ===\n", reply.machine);
    let width = reply.machines.iter().map(|node| node.name.len()).max().unwrap_or(0);
    for node in &reply.machines {
        let state = match node.online {
            _ if node.name == reply.machine => "this machine".to_string(),
            _ if node.revoked.is_some() => {
                let revoked = node.revoked.as_ref().expect("checked");
                format!("revoked by {} at {}: {}", revoked.by, clock(revoked.at, now), revoked.reason)
            }
            Some(true) => match node.since {
                Some(since) => format!("online since {}", clock(since, now)),
                None => "online".to_string(),
            },
            Some(false) => match node.since {
                Some(since) => format!("offline since {}", clock(since, now)),
                None => "offline".to_string(),
            },
            None => "not known".to_string(),
        };
        let facts = match &node.facts {
            Ok(facts) => format!(
                "{} {} · home {} · temp {} · PATH separator {} · file names {} · fastctx {}",
                facts.os,
                facts.arch,
                facts.home,
                facts.temp,
                facts.path_separator,
                if facts.case_sensitive { "case-sensitive" } else { "ignore case" },
                facts.version
            ),
            Err(_) if node.online == Some(false) || node.revoked.is_some() => String::new(),
            Err(error) => format!("not known: {error}"),
        };
        let line = format!("{:width$}  {state}", node.name);
        if facts.is_empty() {
            text.push_str(&format!("{line}\n"));
        } else {
            text.push_str(&format!("{line}  {facts}\n"));
        }
    }
    text
}

/// A moment as the local clock reads it: the time today, the date before.
fn clock(at: Timestamp, now: Timestamp) -> String {
    let zone = jiff::tz::TimeZone::system();
    let (at, now) = (at.to_zoned(zone.clone()), now.to_zoned(zone));
    if at.date() == now.date() { at.strftime("%H:%M").to_string() } else { at.strftime("%Y-%m-%d %H:%M").to_string() }
}
