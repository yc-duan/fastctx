//! The commands that put this computer on hubs and look after its accounts
//! (`world-network.md`): logging in and out, tokens for other devices, what
//! an account's machines let other accounts do, direct connections, and the
//! hub itself.

use clap::{Args, Subcommand};
use fastctx_types::accounts::{AccountChange, AccountsReply, MemberKind};
use fastctx_types::world::{HubNodes, LinkState, NodesReply, Path};
use fastctx_types::{AccountName, Call, HubName, MachineName, Reply};
use jiff::{SignedDuration, Timestamp};

#[derive(Args)]
pub struct LoginArgs {
    /// The hub, as host:port.
    #[arg(required_unless_present = "token")]
    hub: Option<String>,
    /// The account.
    #[arg(required_unless_present = "token")]
    account: Option<AccountName>,
    /// Log in with a one-time token another device made instead.
    #[arg(long, conflicts_with_all = ["hub", "account", "open"])]
    token: Option<String>,
    /// Open the account first, on a hub that lets anyone.
    #[arg(long)]
    open: bool,
    /// What this computer is called on the hub; its host name when not
    /// given. It has this name on every hub it is on.
    #[arg(long)]
    name: Option<MachineName>,
}

#[derive(Args)]
pub struct LogoutArgs {
    account: AccountName,
    /// The hub, when this computer is on more than one.
    #[arg(long)]
    hub: Option<HubName>,
}

#[derive(Args)]
pub struct TokenArgs {
    /// The account it logs in to.
    account: AccountName,
    #[arg(long)]
    hub: Option<HubName>,
    /// What logs in with it: a phone or another computer only logs in; for
    /// a server, the line that installs it and registers it as a machine
    /// is printed instead.
    #[arg(long = "for", default_value = "computer", value_parser = ["phone", "computer", "server"])]
    used_by: String,
    /// How many hours it stays valid.
    #[arg(long, default_value_t = 1, value_parser = hours)]
    hours: u32,
    /// For a server: the name it takes.
    #[arg(long)]
    name: Option<MachineName>,
}

#[derive(Args)]
pub struct ShareArgs {
    /// One of the account's machines.
    machine: MachineName,
    /// The account it is shared with.
    to: AccountName,
    /// Take the share back.
    #[arg(long)]
    off: bool,
    /// The account the machine is registered under; the one this computer
    /// is logged in to when not given.
    #[arg(long = "as")]
    owner: Option<AccountName>,
    #[arg(long)]
    hub: Option<HubName>,
}

#[derive(Args)]
pub struct LendArgs {
    /// One of the account's machines.
    machine: MachineName,
    /// The account the login is lent to.
    to: AccountName,
    /// The agent: codex or claude.
    #[arg(long)]
    agent: fastctx_types::AgentName,
    /// The login's directory on that machine (its CODEX_HOME or
    /// CLAUDE_CONFIG_DIR); the agent's default login there when not given.
    #[arg(long)]
    home: Option<std::path::PathBuf>,
    /// Take the login back.
    #[arg(long)]
    off: bool,
    #[arg(long = "as")]
    owner: Option<AccountName>,
    #[arg(long)]
    hub: Option<HubName>,
}

#[derive(Args)]
pub struct DisallowArgs {
    /// A machine of another account whose agents this account used.
    machine: MachineName,
    #[arg(long = "as")]
    account: Option<AccountName>,
    #[arg(long)]
    hub: Option<HubName>,
}

#[derive(Args)]
pub struct RemoveArgs {
    /// A device logged in to the account, or one of its machines.
    name: MachineName,
    #[arg(long = "as")]
    account: Option<AccountName>,
    #[arg(long)]
    hub: Option<HubName>,
    #[arg(long, default_value = "removed by the user")]
    reason: String,
}

#[derive(Args)]
pub struct DirectArgs {
    /// The machine other machines are to connect to directly.
    machine: MachineName,
    /// An address other machines reach it at, as IP or IP:PORT; give it once
    /// for each. Without any, the addresses it may be reached at are listed.
    #[arg(long = "at", conflicts_with = "off")]
    at: Vec<String>,
    /// Other machines no longer connect to it directly.
    #[arg(long)]
    off: bool,
    #[arg(long = "as")]
    account: Option<AccountName>,
    #[arg(long)]
    hub: Option<HubName>,
}

#[derive(Subcommand)]
pub enum HubCommand {
    /// Create a hub on this machine, with its first account, its super
    /// administrator, whose password is asked for.
    Init {
        /// The hub's short name: letters and digits of any language, and
        /// hyphens.
        #[arg(long)]
        name: HubName,
        /// The super administrator's account.
        #[arg(long)]
        admin: AccountName,
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
        /// Run it as a system service from now on, asking for administrator
        /// rights where they are needed; on Windows other computers are let
        /// through the firewall to it.
        #[arg(long)]
        service: bool,
    },
    /// Run the hub in the foreground.
    Serve {
        #[arg(long, value_parser = crate::paths::parser())]
        dir: Option<std::path::PathBuf>,
    },
    /// Set an account's password from the hub's own machine, as for a
    /// super administrator who forgot theirs. The hub must not be running.
    Password {
        account: AccountName,
        #[arg(long, value_parser = crate::paths::parser())]
        dir: Option<std::path::PathBuf>,
    },
    /// Make the hub kept in --dir, of the network's third version, a hub of
    /// the fourth: its id and certificate stay, so every machine reconnects
    /// with the key it has; its machines and devices go into one new
    /// account, its super administrator, whose password is asked for. The third
    /// version's files are kept beside it. The hub must not be running.
    Migrate {
        /// The hub's short name: letters and digits of any language, and
        /// hyphens.
        #[arg(long)]
        name: HubName,
        /// The account the machines go into.
        #[arg(long)]
        admin: AccountName,
        #[arg(long, value_parser = crate::paths::parser())]
        dir: Option<std::path::PathBuf>,
    },
    /// The hub's name, id, addresses and certificate pin.
    Show {
        #[arg(long, value_parser = crate::paths::parser())]
        dir: Option<std::path::PathBuf>,
    },
}

/// A computer that logs in joins as one of the account's machines
/// (`world-network.md`, 一台电脑和中枢): there is no logging in without it.
pub fn login(args: LoginArgs) -> anyhow::Result<()> {
    let reply = match args.token {
        Some(token) => call(Call::LoginWithToken { token, name: args.name })?,
        None => {
            let (hub, account) = (args.hub.expect("clap requires it"), args.account.expect("clap requires it"));
            let addresses = runtime()?.block_on(fastctx_client::hub_addresses(&hub)).map_err(anyhow::Error::msg)?;
            if args.open {
                let password = crate::secret::new_password(&format!("A password for {account}: "))?;
                call(Call::OpenAccount { hub: addresses, account, password, register: true, name: args.name })?
            } else {
                let password = crate::secret::read_password(&format!("{account}'s password: "))?;
                call(Call::Login { hub: addresses, account, password, register: true, name: args.name })?
            }
        }
    };
    match reply {
        Reply::LoggedIn { hub, name, account } => {
            println!("logged in to {account} on the hub {hub}; this computer is {name} there")
        }
        other => anyhow::bail!("the node answered something else: {other:?}"),
    }
    crate::keep::keep_connected()
}

pub fn logout(args: LogoutArgs) -> anyhow::Result<()> {
    change(args.hub, args.account.clone(), AccountChange::Logout)?;
    println!("logged out of {}", args.account);
    Ok(())
}

pub fn token(args: TokenArgs) -> anyhow::Result<()> {
    let expires = Timestamp::now() + SignedDuration::from_hours(i64::from(args.hours));
    let until = expires.to_zoned(jiff::tz::TimeZone::system()).strftime("%Y-%m-%d %H:%M %Z");
    if args.used_by == "server" {
        match call(Call::ServerCommand { hub: args.hub, account: args.account, name: args.name, expires })? {
            Reply::ServerInvitation { command } => {
                println!("{command}");
                eprintln!(
                    "Paste this line into the server's terminal, as the user it is for. It works once, until {until}."
                );
            }
            other => anyhow::bail!("the node answered something else: {other:?}"),
        }
        return Ok(());
    }
    let kind = if args.used_by == "phone" { MemberKind::Phone } else { MemberKind::Computer };
    let token = change(
        args.hub,
        args.account,
        AccountChange::Token { register: kind == MemberKind::Computer, name: None, kind, expires },
    )?
    .ok_or_else(|| anyhow::anyhow!("the hub made no token"))?;
    println!("{token}");
    eprintln!(
        "On the other device, run `fastctx login --token <token>`, or scan it in the World app. It works once, until {until}."
    );
    Ok(())
}

pub fn share(args: ShareArgs) -> anyhow::Result<()> {
    let owner = account_or_owner(args.hub.as_ref(), args.owner, Some(&args.machine))?;
    let change = AccountChange::Share { machine: args.machine.clone(), to: args.to.clone(), shared: !args.off };
    self::change(args.hub, owner, change)?;
    if args.off {
        println!("{} no longer shares {}", args.to, args.machine);
    } else {
        println!(
            "{} is shared with {}: it may work on it and use every agent on it, and you see all it does there",
            args.machine, args.to
        );
    }
    Ok(())
}

pub fn lend(args: LendArgs) -> anyhow::Result<()> {
    let owner = account_or_owner(args.hub.as_ref(), args.owner, Some(&args.machine))?;
    let login = args.home.as_ref().map(|home| home.display().to_string()).unwrap_or_else(|| "default".to_string());
    let change = AccountChange::Lend {
        machine: args.machine.clone(),
        to: args.to.clone(),
        agent: args.agent.clone(),
        home: args.home,
        lent: !args.off,
    };
    self::change(args.hub, owner, change)?;
    if args.off {
        println!("the {} login {login} on {} is no longer lent to {}", args.agent, args.machine, args.to);
    } else {
        println!(
            "the {} login {login} on {} is lent to {}: the account and its plan go with it, not the machine, and you see all it does there",
            args.agent, args.machine, args.to
        );
    }
    Ok(())
}

pub fn disallow(args: DisallowArgs) -> anyhow::Result<()> {
    let account = account_or_owner(args.hub.as_ref(), args.account, None)?;
    self::change(args.hub, account.clone(), AccountChange::StopActing { machine: args.machine.clone() })?;
    println!("the agents on {} no longer reach {account}'s machines", args.machine);
    Ok(())
}

pub fn remove(args: RemoveArgs) -> anyhow::Result<()> {
    let account = account_or_owner(args.hub.as_ref(), args.account, Some(&args.name))?;
    self::change(args.hub, account.clone(), AccountChange::Remove { name: args.name.clone(), reason: args.reason })?;
    println!("{} is out of {account}", args.name);
    Ok(())
}

pub fn direct(args: DirectArgs) -> anyhow::Result<()> {
    let account = account_or_owner(args.hub.as_ref(), args.account, None)?;
    if args.off {
        self::change(args.hub, account, AccountChange::Direct { machine: args.machine.clone(), at: Vec::new() })?;
        println!("other machines no longer connect to {} directly", args.machine);
        return Ok(());
    }
    if args.at.is_empty() {
        let Reply::Accounts(reply) = call(Call::Accounts)? else { anyhow::bail!("the node answered something else") };
        let member = reply
            .hubs
            .iter()
            .filter(|seen| args.hub.as_ref().is_none_or(|hub| seen.name == *hub))
            .find_map(|seen| seen.members.iter().find(|member| member.name == args.machine))
            .ok_or_else(|| anyhow::anyhow!("there is no machine {} on the hubs this computer is on", args.machine))?;
        let reach = member.reach.clone().unwrap_or_default();
        let mut text = format!("{} may be reached at:\n", args.machine);
        if let Some(seen) = reach.seen {
            text.push_str(&format!("  {seen}  (where the hub sees it)\n"));
        }
        for address in &reach.interfaces {
            text.push_str(&format!("  {address}  (one of its network cards)\n"));
        }
        if reach.seen.is_none() && reach.interfaces.is_empty() {
            text.push_str("  nothing is known: it is offline, or this account is not the network's administrator\n");
        }
        print!("{text}");
        anyhow::bail!(
            "choose with --at ADDRESS[:PORT]; the port is {} when none is given",
            fastctx_net::peer::DEFAULT_PORT
        );
    }
    let mut at = Vec::new();
    for text in &args.at {
        let address = match text.parse::<std::net::SocketAddr>() {
            Ok(address) => address,
            Err(_) => std::net::SocketAddr::new(
                text.parse().map_err(|_| anyhow::anyhow!("{text} is not an IP address, with or without a port"))?,
                fastctx_net::peer::DEFAULT_PORT,
            ),
        };
        at.push(address);
    }
    self::change(args.hub, account, AccountChange::Direct { machine: args.machine.clone(), at: at.clone() })?;
    let list = at.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ");
    println!("other machines connect to {} directly at {list}", args.machine);
    for address in at.iter().filter(|address| fastctx_net::peer::local_only(&address.ip())) {
        println!("{address} is reached only on its own network; the hub could not check it from where it is");
    }
    Ok(())
}

pub fn accounts() -> anyhow::Result<()> {
    match call(Call::Accounts)? {
        Reply::Accounts(reply) => {
            print!("{}", render_accounts(&reply, Timestamp::now()));
            // In a network, this computer is to be there from boot
            // (`world-network.md`, 登记了，就一直连着); when the service that
            // does it was never installed, the user is told so.
            let world = fastctx_types::layout::WorldDir::under(&fastctx_platform::fastctx_root()?);
            if !reply.hubs.is_empty() && !world.node_service().try_exists()? {
                println!(
                    "\nno system service keeps this computer's node: it runs only while something here started it. \
                     `fastctx service node install`, as administrator, installs one."
                );
            }
        }
        other => anyhow::bail!("the node answered something else: {other:?}"),
    }
    Ok(())
}

pub fn nodes() -> anyhow::Result<()> {
    match call(Call::Nodes)? {
        Reply::Nodes(reply) => crate::output::print(&render_nodes(&reply, Timestamp::now()))?,
        other => anyhow::bail!("the node answered something else: {other:?}"),
    }
    Ok(())
}

pub fn hub(command: HubCommand) -> anyhow::Result<()> {
    let root = fastctx_platform::fastctx_root()?;
    let dir = |dir: Option<std::path::PathBuf>| dir.unwrap_or_else(|| fastctx_hub::default_dir(&root));
    match command {
        HubCommand::Init { name, admin, public, listen, dir: chosen, service } => {
            let dir = dir(chosen);
            let listen = listen.unwrap_or_else(|| std::net::SocketAddr::from(([0, 0, 0, 0], public[0].port())));
            let password = crate::secret::new_password(&format!("A password for {admin}: "))?;
            let new =
                fastctx_hub::New { name: name.clone(), public: public.clone(), listen, admin: admin.clone(), password };
            fastctx_hub::init(&dir, new)?;
            let at = public.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ");
            println!("the hub {name} is in {}; its super administrator is {admin}", dir.display());
            let started = if service { keep_hub(&dir)? } else { false };
            if started {
                println!("it runs as a system service, reached at {at}");
                eprintln!("Log in from a computer with `fastctx login {} {admin}`.", public[0]);
            } else {
                eprintln!(
                    "Start it with `fastctx hub serve` (or install its service); then log in from a computer with `fastctx login {} {admin}`. It is reached at {at}.",
                    public[0]
                );
            }
        }
        HubCommand::Serve { dir: chosen } => {
            let dir = dir(chosen);
            tokio::runtime::Builder::new_multi_thread().enable_all().build()?.block_on(fastctx_hub::serve(&dir))?;
        }
        HubCommand::Password { account, dir: chosen } => {
            let password = crate::secret::new_password(&format!("A new password for {account}: "))?;
            fastctx_hub::reset_password(&dir(chosen), &account, &password)?;
            println!("{account}'s password is set; devices already logged in stay logged in");
        }
        HubCommand::Migrate { name, admin, dir: chosen } => {
            let dir = dir(chosen);
            let file_name = dir.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
            let fourth = dir.with_file_name(format!("{file_name}-fourth"));
            let third = dir.with_file_name(format!("{file_name}-third"));
            if third.exists() {
                anyhow::bail!("{} exists already: the hub was migrated once", third.display());
            }
            let password = crate::secret::new_password(&format!("A password for {admin}: "))?;
            let migrated = fastctx_hub::third::Migrated { name: name.clone(), admin: admin.clone(), password };
            let report = fastctx_hub::third::migrate(&dir, &fourth, migrated)?;
            std::fs::rename(&dir, &third)?;
            std::fs::rename(&fourth, &dir)?;
            let list = |names: &[MachineName]| names.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ");
            println!(
                "the hub {name} in {} is of the fourth version; its super administrator is {admin}",
                dir.display()
            );
            println!("machines of {admin}: {}", list(&report.machines));
            if !report.devices.is_empty() {
                println!("devices logged in to {admin}: {}", list(&report.devices));
            }
            for (member, why) in &report.left {
                println!("left out {member}: {why}");
            }
            if report.letters > 0 {
                println!("{} letters the third version still held stay in {}", report.letters, third.display());
            }
            eprintln!(
                "The third version's files are in {}. Start the hub again; each machine reconnects once it runs the fourth version.",
                third.display()
            );
        }
        HubCommand::Show { dir: chosen } => {
            let (hub, pin) = fastctx_hub::describe(&dir(chosen))?;
            let public = hub.public.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ");
            println!(
                "hub {} ({})\nreached at {public}\nlistens on {}\ncertificate {pin}",
                hub.name, hub.id, hub.listen
            );
            if hub.self_registration {
                println!("anyone who reaches it may open an account");
            }
        }
    }
    Ok(())
}

/// Runs the hub in `dir` as a system service; whether it runs now.
fn keep_hub(dir: &std::path::Path) -> anyhow::Result<bool> {
    use fastctx_install::service::Kept;
    let places = fastctx_install::apply::Places::here()?;
    match fastctx_install::service::keep_hub(&places, dir)? {
        Kept::Installed | Kept::Already => Ok(true),
        Kept::Declined => anyhow::bail!("administrator rights were declined; the hub is made but not running"),
        Kept::Command(command) => {
            eprintln!("Its service needs administrator rights: {command}");
            sudo_hub(&places, dir)
        }
        Kept::Unsupported => {
            eprintln!("This system has no service for it.");
            Ok(false)
        }
    }
}

/// Installs the hub's service through sudo, as `server install` does the
/// node's: asked at the terminal, or without one read from standard input.
#[cfg(unix)]
fn sudo_hub(places: &fastctx_install::apply::Places, dir: &std::path::Path) -> anyhow::Result<bool> {
    use std::io::IsTerminal as _;
    let (executable, _) = fastctx_install::apply::place_executable(places)?;
    let mut sudo = std::process::Command::new("sudo");
    if !std::io::stdin().is_terminal() {
        sudo.args(["-S", "-p", ""]);
    }
    let status = sudo
        .arg(&executable)
        .args(["service", "hub", "install", "--dir"])
        .arg(dir)
        .arg("--executable")
        .arg(&executable)
        .status()
        .map_err(|error| anyhow::anyhow!("starting sudo for the hub's service: {error}"))?;
    anyhow::ensure!(status.success(), "sudo service registration failed ({status}); run the command above to finish");
    Ok(true)
}

#[cfg(not(unix))]
fn sudo_hub(_: &fastctx_install::apply::Places, _: &std::path::Path) -> anyhow::Result<bool> {
    Ok(false)
}

/// Makes `change` as `account`, on `hub` or the one hub; a token, when it
/// made one.
fn change(hub: Option<HubName>, account: AccountName, change: AccountChange) -> anyhow::Result<Option<String>> {
    match call(Call::Account { hub, account, change })? {
        Reply::Changed { token } => Ok(token),
        other => anyhow::bail!("the node answered something else: {other:?}"),
    }
}

/// The account a change is made as: the one named, or the one that holds
/// `machine` among this computer's logins, or this computer's one login.
fn account_or_owner(
    hub: Option<&HubName>,
    named: Option<AccountName>,
    machine: Option<&MachineName>,
) -> anyhow::Result<AccountName> {
    if let Some(named) = named {
        return Ok(named);
    }
    let Reply::Accounts(reply) = call(Call::Accounts)? else { anyhow::bail!("the node answered something else") };
    let hubs: Vec<_> = reply.hubs.iter().filter(|seen| hub.is_none_or(|hub| seen.name == *hub)).collect();
    let [seen] = hubs.as_slice() else {
        anyhow::bail!("this computer is on {} hubs; name one with --hub", hubs.len());
    };
    if let Some(machine) = machine
        && let Some(owner) = seen.members.iter().find(|member| member.name == *machine).and_then(|member| {
            member.home.clone().or_else(|| member.logins.iter().find(|login| seen.logins.contains(login)).cloned())
        })
        && seen.logins.contains(&owner)
    {
        return Ok(owner);
    }
    match seen.logins.as_slice() {
        [only] => Ok(only.clone()),
        [] => anyhow::bail!("this computer is logged in to no account on {}", seen.name),
        _ => anyhow::bail!("this computer is logged in to several accounts on {}; name one with --as", seen.name),
    }
}

/// A token stays valid an hour, a day or three.
fn hours(text: &str) -> Result<u32, String> {
    match text {
        "1" | "24" | "72" => Ok(text.parse().expect("listed")),
        _ => Err("a token stays valid 1, 24 or 72 hours".to_string()),
    }
}

fn call(call: Call) -> anyhow::Result<Reply> {
    Ok(crate::ask::call(call)?.0)
}

fn runtime() -> anyhow::Result<tokio::runtime::Runtime> {
    Ok(tokio::runtime::Builder::new_current_thread().enable_all().build()?)
}

fn link(link: &LinkState, now: Timestamp) -> String {
    // A model names machines and never needs where the hub or any machine
    // is on the network: in a script run for an agent the link says only
    // whether it is up, for how long, and through which machine.
    if std::env::var_os("FASTCTX_RESIDENT").is_some() {
        return match link {
            LinkState::Alone => "not connected".to_string(),
            LinkState::Online { since, through: None, .. } => format!("connected for {}", age(*since, now)),
            LinkState::Online { since, through: Some(through), .. } => {
                format!("connected through {through}, for {}", age(*since, now))
            }
            LinkState::Connecting { since, .. } => format!("not connected to the hub for {}", age(*since, now)),
            LinkState::Stopped { reason } => format!("the hub removed this computer: {reason}"),
        };
    }
    match link {
        LinkState::Alone => "not connected".to_string(),
        LinkState::Online { since, hub, through: None, route, ms } => {
            format!("connected to the hub at {hub}, {route}, for {}{}", age(*since, now), round_trip(*ms))
        }
        LinkState::Online { since, hub, through: Some(through), route, ms } => {
            format!("connected through {through} at {hub}, {route}, for {}{}", age(*since, now), round_trip(*ms))
        }
        LinkState::Connecting { last_error: None, .. } => "connecting to the hub".to_string(),
        LinkState::Connecting { since, last_error: Some(error) } => {
            format!("not connected to the hub for {}: {error}", age(*since, now))
        }
        LinkState::Stopped { reason } => format!("the hub removed this computer: {reason}"),
    }
}

fn render_nodes(reply: &NodesReply, now: Timestamp) -> String {
    let mut text = format!("=== nodes: {} ===\n", reply.machine);
    if let Some(facts) = &reply.facts {
        text.push_str(&format!("{}  this computer  {}\n", reply.machine, facts_line(facts)));
    }
    if reply.hubs.is_empty() {
        text.push_str("in no network: this computer is the only machine\n");
    }
    for hub in &reply.hubs {
        text.push_str(&hub_nodes(hub, now));
    }
    text
}

/// A round trip as a person reads it after a line's state.
fn round_trip(ms: Option<u32>) -> String {
    ms.map(|ms| format!(", {ms} ms round trip")).unwrap_or_default()
}

fn hub_nodes(hub: &HubNodes, now: Timestamp) -> String {
    let resident = std::env::var_os("FASTCTX_RESIDENT").is_some();
    let logins = hub.logins.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ");
    let home = hub.home.as_ref().map(|home| format!("; registered under {home}")).unwrap_or_default();
    let mut text = format!("\n--- hub {} ({}) · logged in to {logins}{home}\n", hub.name, link(&hub.link, now));
    if hub.machines.is_empty() {
        text.push_str(&format!("no other machine is in {}: this computer is the only machine\n", hub.name));
    }
    // Names line up by the columns they take in a terminal: a Chinese
    // character takes two.
    let width = hub.machines.iter().map(|node| fastctx_tui::text::width(&node.written.to_string())).max().unwrap_or(0);
    for node in &hub.machines {
        let state = match (node.online, node.since) {
            (Some(true), Some(since)) => format!("online for {}", age(since, now)),
            (Some(true), None) => "online".to_string(),
            (Some(false), Some(since)) => format!("offline for {}", age(since, now)),
            (Some(false), None) => "offline".to_string(),
            (None, _) => "not known".to_string(),
        };
        let through = node.through.as_ref().map(|through| format!(" · through {through}")).unwrap_or_default();
        // How this computer's calls to it go: which way, never where.
        let path = match &node.path {
            Some(Path::Direct { .. }) if resident => " · direct".to_string(),
            Some(Path::Direct { ms }) => format!(" · direct{}", round_trip(*ms)),
            Some(Path::Hub { why: Some(why) }) if !resident => format!(" · no direct link: {why}"),
            _ => String::new(),
        };
        let owner = node.owner.as_ref().map(|owner| format!(" · {owner}'s")).unwrap_or_default();
        let rights: Vec<String> = node
            .rights
            .iter()
            .filter(|(_, rights)| rights.any())
            .map(|(account, rights)| match (rights.hands, rights.agents) {
                (true, true) => format!("{account}: work, agents"),
                (true, false) => format!("{account}: work"),
                _ => format!("{account}: agents"),
            })
            .collect();
        let facts = match &node.facts {
            Ok(facts) => format!("  {}", facts_line(facts)),
            Err(_) if node.online == Some(false) => String::new(),
            Err(error) => format!("  not known: {error}"),
        };
        let written = node.written.to_string();
        let pad = " ".repeat(width - fastctx_tui::text::width(&written));
        text.push_str(&format!("{written}{pad}  {state}{through}{path}{owner} · {}{facts}\n", rights.join("; ")));
        if let Ok(facts) = &node.facts
            && let Some(warning) = facts.node_lifetime.as_ref().and_then(|lifetime| lifetime.warning())
        {
            text.push_str(&format!("  {warning}\n"));
        }
    }
    text
}

fn facts_line(facts: &fastctx_types::world::MachineFacts) -> String {
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

fn render_accounts(reply: &AccountsReply, now: Timestamp) -> String {
    let mut text = format!("=== accounts: {} ===\n", reply.machine);
    if reply.hubs.is_empty() {
        text.push_str("logged in to no hub\n");
    }
    for hub in &reply.hubs {
        let logins = hub.logins.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ");
        text.push_str(&format!("\n--- hub {} ({}) · logged in to {logins}\n", hub.name, link(&hub.link, now)));
        for arrival in &hub.arrivals {
            let what = if arrival.device { "logged in to" } else { "was registered under" };
            text.push_str(&format!(
                "new: {} {what} {} {} ago; `fastctx remove {} --as {}` if it was not you\n",
                arrival.name,
                arrival.account,
                age(arrival.noticed_at, now),
                arrival.name,
                arrival.account
            ));
        }
        for account in &hub.accounts {
            let mut marks = Vec::new();
            if account.level == fastctx_types::accounts::Level::Super {
                marks.push("super administrator");
            }
            if account.locked {
                marks.push("locked");
            }
            let marks = if marks.is_empty() { String::new() } else { format!(" ({})", marks.join(", ")) };
            text.push_str(&format!("account {}{marks}\n", account.name));
            for member in hub.members.iter().filter(|member| member.home.as_ref() == Some(&account.name)) {
                text.push_str(&format!("  machine {} [{}]\n", member.name, member.key));
            }
            for member in hub
                .members
                .iter()
                .filter(|member| member.home.as_ref() != Some(&account.name) && member.logins.contains(&account.name))
            {
                text.push_str(&format!("  logged in on {} [{}]\n", member.name, member.key));
            }
        }
        for share in &hub.shares {
            text.push_str(&format!("shared {} with {}\n", share.machine, share.account));
        }
        for lend in &hub.lends {
            let login =
                lend.home.as_ref().map(|home| home.display().to_string()).unwrap_or_else(|| "default".to_string());
            text.push_str(&format!("lent the {} login {login} on {} to {}\n", lend.agent, lend.machine, lend.account));
        }
        for acting in &hub.acting {
            text.push_str(&format!(
                "the agents on {} reach {}'s machines, for {}\n",
                acting.machine,
                acting.account,
                age(acting.since, now)
            ));
        }
        for project in &hub.projects {
            let granted = if project.granted.is_empty() {
                String::new()
            } else {
                format!(
                    ", shared with {}",
                    project.granted.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ")
                )
            };
            text.push_str(&format!("project {} of {}{granted}\n", project.name, project.owner));
        }
    }
    text
}

/// How long ago a moment was, written as `w` writes it: the machines of a
/// World keep different clocks and zones, and a reader may know none of them.
fn age(at: Timestamp, now: Timestamp) -> String {
    fastctx_hands::told::duration(now.as_second().saturating_sub(at.as_second()).max(0) as u64)
}
