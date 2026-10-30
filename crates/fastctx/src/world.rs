//! The commands that put this computer on hubs and look after its accounts
//! (`world-network.md`): logging in and out, tokens for other devices, what
//! an account's machines let other accounts do, direct connections, and the
//! hub itself.

use clap::{Args, Subcommand};
use fastctx_types::accounts::{AccountChange, AccountsReply, HubAccounts, Level, MemberKind, TokenSeen, TokenWay};
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
    /// The HTTP proxy this computer reaches its hubs through, as
    /// http://host:port, kept in Fastctx before it logs in.
    #[arg(long, value_parser = crate::proxy::parser)]
    proxy: Option<String>,
}

#[derive(Args)]
pub struct LogoutArgs {
    account: AccountName,
    /// The hub, when this computer is on more than one.
    #[arg(long)]
    hub: Option<HubName>,
}

#[derive(Subcommand)]
pub enum TokenCommand {
    /// Make a code that lets other devices into an account: a phone or a
    /// computer logs in with it; for a server, the line that installs it
    /// and registers it as a machine is printed instead.
    New(TokenArgs),
    /// The codes that still let devices in, of the accounts this computer
    /// is logged in to, and every account's for the network's
    /// administrator: what each is for, how many came in by it, until when,
    /// and its key.
    List {
        /// The hub, when this computer is on more than one.
        #[arg(long)]
        hub: Option<HubName>,
    },
    /// Revoke a code: no device comes in by it any more; those that did
    /// stay.
    Revoke {
        /// The code: its key, the start of it of six characters or more, or
        /// its name.
        code: String,
        /// The hub, when this computer is on more than one.
        #[arg(long)]
        hub: Option<HubName>,
    },
}

#[derive(Args)]
pub struct TokenArgs {
    /// The account it logs in to.
    account: AccountName,
    #[arg(long)]
    hub: Option<HubName>,
    /// What logs in with it: a phone or another computer only logs in; for
    /// a Linux server, a Windows computer or a Mac, the line that installs it
    /// and registers it as a machine is printed instead.
    #[arg(long = "for", default_value = "computer", value_parser = ["phone", "computer", "server", "windows", "mac"])]
    used_by: String,
    /// How long it stays valid: hours or days, as 1h or 7d, up to a year,
    /// or `never`, until it is revoked. An hour for a phone or a computer,
    /// a day for a server and a week for managed machines when not given.
    #[arg(long, value_parser = valid)]
    valid: Option<Valid>,
    /// For a code used once: the name the device takes.
    #[arg(long)]
    name: Option<MachineName>,
    /// For a computer or server: the computers that use it join as managed
    /// machines of the account, logged in to none.
    #[arg(long)]
    managed: bool,
    /// How many devices may use it: a number up to 9999, or `any`. One when
    /// not given; each of several takes its own name.
    #[arg(long, value_parser = uses)]
    uses: Option<u32>,
    /// A name for it, as 「机房 A」; the machines that come in by a code for
    /// several are put in the group of its name.
    #[arg(long)]
    label: Option<String>,
}

/// How long a code stays valid.
#[derive(Clone, Copy)]
pub enum Valid {
    For(SignedDuration),
    /// Until it is revoked.
    Never,
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
    /// The most jobs the account runs on it at once; no limit when not
    /// given. Sharing again sets it anew.
    #[arg(long, conflicts_with = "off", value_parser = clap::value_parser!(u16).range(1..))]
    jobs: Option<u16>,
    /// The account the machine is registered under; the one this computer
    /// is logged in to when not given.
    #[arg(long = "as")]
    owner: Option<AccountName>,
    #[arg(long)]
    hub: Option<HubName>,
}

#[derive(Args)]
pub struct AssistArgs {
    /// One of the account's machines.
    machine: MachineName,
    /// The account the login assists.
    to: AccountName,
    /// The agent: codex or claude.
    #[arg(long)]
    agent: fastctx_types::AgentName,
    /// The login's directory on that machine (its CODEX_HOME or
    /// CLAUDE_CONFIG_DIR); the agent's default login there when not given.
    #[arg(long)]
    home: Option<std::path::PathBuf>,
    /// End the assistance.
    #[arg(long)]
    off: bool,
    /// When the assistance ends by itself: a date and time on this
    /// computer's clock, such as "2026-10-20 18:00", or a full timestamp.
    /// Given again without it, the login assists until the assistance is
    /// ended.
    #[arg(long, conflicts_with = "off", value_parser = deadline)]
    until: Option<Timestamp>,
    /// What the account may spend of a Claude Code login in a week of the
    /// login's own seven-day window, in US dollars as its cost ledger
    /// counts them, such as 50; no limit when not given.
    #[arg(long, conflicts_with = "off", value_parser = dollars)]
    weekly: Option<fastctx_types::usage::Usd>,
    /// What the account may spend of a Claude Code login in any 24 hours,
    /// in US dollars, such as 10; no limit when not given.
    #[arg(long, conflicts_with = "off", value_parser = dollars)]
    daily: Option<fastctx_types::usage::Usd>,
    /// The share of the login's own five-hour window, in percent from 1 to
    /// 100, past which the login assists no one, its owner's own use
    /// counted too; no limit when not given.
    #[arg(long = "five-hour", conflicts_with = "off", value_parser = clap::value_parser!(u8).range(1..=100))]
    five_hour: Option<u8>,
    /// The most turns the login runs at once, its owner's own counted too,
    /// the same on every assistance of the login; no limit of yours when
    /// not given.
    #[arg(long, conflicts_with = "off", value_parser = clap::value_parser!(u16).range(1..))]
    turns: Option<u16>,
    #[arg(long = "as")]
    owner: Option<AccountName>,
    #[arg(long)]
    hub: Option<HubName>,
}

/// An amount of US dollars as typed, such as 50 or 12.5.
fn dollars(text: &str) -> Result<fastctx_types::usage::Usd, String> {
    let amount: f64 =
        text.trim().trim_start_matches('$').parse().map_err(|_| format!("{text} is not an amount of dollars"))?;
    fastctx_types::usage::Usd::from_dollars(amount)
        .filter(|usd| usd.0 > 0)
        .ok_or_else(|| format!("{text} is not an amount of dollars above zero"))
}

/// A deadline as typed: a timestamp, or a date and time on this
/// computer's clock.
fn deadline(text: &str) -> Result<Timestamp, String> {
    if let Ok(at) = text.parse::<Timestamp>() {
        return Ok(at);
    }
    let civil = text
        .parse::<jiff::civil::DateTime>()
        .map_err(|_| format!("{text:?} is not a date and time such as \"2026-10-20 18:00\""))?;
    civil.to_zoned(jiff::tz::TimeZone::system()).map(|zoned| zoned.timestamp()).map_err(|error| error.to_string())
}

/// `at` on this computer's clock.
fn local(at: Timestamp) -> String {
    at.to_zoned(jiff::tz::TimeZone::system()).strftime("%Y-%m-%d %H:%M %Z").to_string()
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
    /// Then uninstall Fastctx on that machine, keeping its data: one of
    /// the account's own without the application, or a managed machine.
    #[arg(long)]
    uninstall: bool,
    /// Then uninstall Fastctx on that machine with everything it wrote.
    #[arg(long, conflicts_with = "uninstall")]
    everything: bool,
}

#[derive(Args)]
pub struct DissolveArgs {
    /// The network's name, typed as it is, to confirm.
    network: HubName,
}

#[derive(Args)]
pub struct RenameArgs {
    /// The machine's name now, or a name it had; with the new name alone,
    /// this computer.
    #[arg(value_name = "MACHINE")]
    first: MachineName,
    /// Its new name.
    #[arg(value_name = "NAME")]
    second: Option<MachineName>,
    #[arg(long = "as")]
    account: Option<AccountName>,
    #[arg(long)]
    hub: Option<HubName>,
}

#[derive(Args)]
pub struct GroupArgs {
    /// The machines to put in the group; with --rename, the group's name now.
    #[arg(value_name = "MACHINE", required = true)]
    machines: Vec<String>,
    /// The group: they are shown together under it.
    #[arg(long, conflicts_with_all = ["none", "rename"])]
    to: Option<String>,
    /// Take them out of their group.
    #[arg(long, conflicts_with = "rename")]
    none: bool,
    /// Rename the group named first to this; empty, end it, its machines in
    /// no group.
    #[arg(long, value_name = "NAME")]
    rename: Option<String>,
    /// The account whose group it is, for --rename; the one this computer is
    /// registered under by default.
    #[arg(long = "as")]
    account: Option<AccountName>,
    #[arg(long)]
    hub: Option<HubName>,
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
    /// End the hub of a dissolved network kept here before every machine
    /// that was on it was told: its system service is removed and its data
    /// deleted, all but its log. Run it as administrator.
    Retire {
        #[arg(long, value_parser = crate::paths::parser())]
        dir: Option<std::path::PathBuf>,
    },
    /// Install the system service of the hub moving to this computer, which
    /// takes administrator rights here; the old hub then hands over to it.
    Arrive {
        #[arg(long, value_parser = crate::paths::parser())]
        dir: Option<std::path::PathBuf>,
    },
}

/// A computer that logs in joins as one of the account's machines
/// (`world-network.md`, 一台电脑和中枢): there is no logging in without it.
pub fn login(args: LoginArgs) -> anyhow::Result<()> {
    if let Some(url) = args.proxy {
        crate::proxy::set(Some(url))?;
    }
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

pub fn token(command: TokenCommand) -> anyhow::Result<()> {
    match command {
        TokenCommand::New(args) => new_token(args),
        TokenCommand::List { hub } => list_tokens(hub),
        TokenCommand::Revoke { code, hub } => revoke_token(&code, hub),
    }
}

fn new_token(args: TokenArgs) -> anyhow::Result<()> {
    let windows = args.used_by == "windows";
    let mac = args.used_by == "mac";
    let server = args.used_by == "server" || windows || mac;
    let kind = if args.used_by == "phone" { MemberKind::Phone } else { MemberKind::Computer };
    if args.managed && kind == MemberKind::Phone {
        anyhow::bail!("a phone is never a managed machine");
    }
    if args.uses.is_some_and(|uses| uses != 1) && args.name.is_some() {
        anyhow::bail!("a code for several devices names none of them: each takes its own name");
    }
    let day = SignedDuration::from_hours(24);
    let valid = args.valid.unwrap_or(Valid::For(if args.managed {
        day * 7
    } else if server {
        day
    } else {
        SignedDuration::from_hours(1)
    }));
    let expires = match valid {
        Valid::Never => fastctx_types::accounts::LASTING,
        Valid::For(span) => Timestamp::now() + span,
    };
    let works = format!("It works {}, {}.", how_many(args.uses.unwrap_or(1)), until(expires));
    if server {
        let reply = call(Call::ServerCommand {
            hub: args.hub,
            account: args.account,
            name: args.name,
            expires,
            managed: args.managed,
            uses: args.uses,
            label: args.label,
            windows,
            mac,
        })?;
        let Reply::ServerInvitation { command } = reply else {
            anyhow::bail!("the node answered something else: {reply:?}")
        };
        println!("{command}");
        if windows {
            eprintln!(
                "Paste this line into each computer's Command Prompt or PowerShell, run as administrator. {works}"
            );
        } else if mac {
            eprintln!("Paste this line into each Mac's Terminal, as root for one that starts at boot. {works}");
        } else {
            eprintln!("Paste this line into each server's terminal, as the user it is for. {works}");
        }
        return Ok(());
    }
    let token = change(
        args.hub,
        args.account,
        AccountChange::Token {
            register: kind == MemberKind::Computer,
            name: args.name,
            kind,
            expires,
            managed: args.managed,
            uses: args.uses,
            label: args.label,
            way: Some(if kind == MemberKind::Phone { TokenWay::Scan } else { TokenWay::Paste }),
        },
    )?
    .ok_or_else(|| anyhow::anyhow!("the hub made no token"))?;
    println!("{token}");
    if args.managed {
        eprintln!(
            "On each computer, in a system account no one at it signs in to, run `fastctx login --token <code>`, or paste it in Join a network in Fastctx. {works}"
        );
    } else {
        eprintln!("On the other device, run `fastctx login --token <code>`, or scan it in the Fastctx app. {works}");
    }
    Ok(())
}

fn list_tokens(hub: Option<HubName>) -> anyhow::Result<()> {
    let Reply::Accounts(reply) = call(Call::Accounts)? else { anyhow::bail!("the node answered something else") };
    let now = Timestamp::now();
    let mut text = String::new();
    for seen in reply.hubs.iter().filter(|seen| hub.as_ref().is_none_or(|hub| seen.name == *hub)) {
        if seen.tokens.is_empty() {
            text.push_str(&format!("no code lets a device into {} now\n", seen.name));
        }
        for token in &seen.tokens {
            text.push_str(&format!("{}\n", token_line(token, now)));
        }
    }
    print!("{text}");
    Ok(())
}

fn revoke_token(code: &str, hub: Option<HubName>) -> anyhow::Result<()> {
    let Reply::Accounts(reply) = call(Call::Accounts)? else { anyhow::bail!("the node answered something else") };
    let named = |token: &TokenSeen| {
        token.key == code || (code.len() >= 6 && token.key.starts_with(code)) || token.label.as_deref() == Some(code)
    };
    let found: Vec<(&HubAccounts, &TokenSeen)> = reply
        .hubs
        .iter()
        .filter(|seen| hub.as_ref().is_none_or(|hub| seen.name == *hub))
        .flat_map(|seen| seen.tokens.iter().filter(|token| named(token)).map(move |token| (seen, token)))
        .collect();
    let [(seen, token)] = found.as_slice() else {
        if found.is_empty() {
            anyhow::bail!("no code that still lets devices in is {code}; `fastctx token list` lists them");
        }
        anyhow::bail!("{} codes are {code}; name one by its key", found.len());
    };
    // Revoked as the account it lets into, or by the network's
    // administrator.
    let account = if seen.logins.contains(&token.account) {
        token.account.clone()
    } else {
        seen.accounts
            .iter()
            .find(|account| account.level == Level::Super && seen.logins.contains(&account.name))
            .map(|account| account.name.clone())
            .ok_or_else(|| anyhow::anyhow!("this computer is not logged in to {}, whose code it is", token.account))?
    };
    change(Some(seen.name.clone()), account, AccountChange::RevokeToken { token: token.key.clone() })?;
    println!("revoked {}; the devices that came in by it stay", what_token(token));
    Ok(())
}

/// A code as `fastctx token list` and `fastctx accounts` say it.
fn token_line(token: &TokenSeen, now: Timestamp) -> String {
    let came = match token.uses {
        0 => format!("{} in, any number may come", token.used),
        uses => format!("{} of {uses} in", token.used),
    };
    let made = match (&token.made_by, token.made_at) {
        (Some(by), Some(at)) => format!(", made on {by} {} ago", age(at, now)),
        _ => String::new(),
    };
    let key: String = token.key.chars().take(10).collect();
    format!("{key}  {} of {}: {came}, {}{made}", what_token(token), token.account, until(token.expires))
}

/// What a code is for, by how it is given and its name.
fn what_token(token: &TokenSeen) -> String {
    let way = match (token.way, token.managed, token.kind) {
        (Some(TokenWay::Line), true, _) => "the server line for managed machines",
        (Some(TokenWay::Line), false, _) => "the server line",
        (Some(TokenWay::Ssh), _, _) => "the code for a server added over SSH",
        (_, true, _) => "the code for managed machines",
        (_, false, MemberKind::Phone) => "the phone code",
        _ => "the computer code",
    };
    match (&token.label, &token.name) {
        (Some(label), _) => format!("{way} {label}"),
        (None, Some(name)) => format!("{way} for {name}"),
        (None, None) => way.to_string(),
    }
}

fn how_many(uses: u32) -> String {
    match uses {
        0 => "for any number of devices".to_string(),
        1 => "once".to_string(),
        uses => format!("for {uses} devices"),
    }
}

fn until(expires: Timestamp) -> String {
    if fastctx_types::accounts::lasting(expires) {
        "until it is revoked".to_string()
    } else {
        format!("until {}", local(expires))
    }
}

pub fn share(args: ShareArgs) -> anyhow::Result<()> {
    let owner = account_or_owner(args.hub.as_ref(), args.owner, Some(&args.machine))?;
    let change =
        AccountChange::Share { machine: args.machine.clone(), to: args.to.clone(), shared: !args.off, jobs: args.jobs };
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

pub fn assist(args: AssistArgs) -> anyhow::Result<()> {
    let owner = account_or_owner(args.hub.as_ref(), args.owner, Some(&args.machine))?;
    let login = args.home.as_ref().map(|home| home.display().to_string()).unwrap_or_else(|| "default".to_string());
    let change = AccountChange::Assist {
        machine: args.machine.clone(),
        to: args.to.clone(),
        agent: args.agent.clone(),
        home: args.home,
        assisting: !args.off,
        until: args.until,
        weekly: args.weekly,
        daily: args.daily,
        five_hour: args.five_hour,
        turns: args.turns,
    };
    self::change(args.hub, owner, change)?;
    if args.off {
        println!("the {} login {login} on {} no longer assists {}", args.agent, args.machine, args.to);
    } else {
        let until = args.until.map(|until| format!(" until {}", local(until))).unwrap_or_default();
        println!(
            "the {} login {login} on {} assists {}{until}: the account and its plan go with it, not the machine, and you see all it does there",
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
    let then = match (args.uninstall, args.everything) {
        (_, true) => Some(fastctx_types::uninstall::Then::Everything),
        (true, false) => Some(fastctx_types::uninstall::Then::Uninstall),
        (false, false) => None,
    };
    let change = AccountChange::Remove { name: args.name.clone(), reason: args.reason, then };
    self::change(args.hub, account.clone(), change)?;
    match then {
        Some(_) => println!("{} is out of {account}, and uninstalls Fastctx once it hears so", args.name),
        None => println!("{} is out of {account}", args.name),
    }
    Ok(())
}

/// Dissolves a network as its super administrator: every machine and device
/// is taken off it, and nobody comes in again.
pub fn dissolve(args: DissolveArgs) -> anyhow::Result<()> {
    let Reply::Accounts(reply) = call(Call::Accounts)? else { anyhow::bail!("the node answered something else") };
    let seen = reply
        .hubs
        .iter()
        .find(|seen| seen.name == args.network)
        .ok_or_else(|| anyhow::anyhow!("this computer is on no network called {}", args.network))?;
    let admin = seen
        .accounts
        .iter()
        .find(|account| account.level == Level::Super && seen.logins.contains(&account.name))
        .ok_or_else(|| anyhow::anyhow!("only the network's super administrator dissolves it"))?;
    change(Some(seen.name.clone()), admin.name.clone(), AccountChange::Dissolve { name: args.network.clone() })?;
    println!("dissolved {}: every machine and device is off it, and nobody comes in again", args.network);
    Ok(())
}

pub fn rename(args: RenameArgs) -> anyhow::Result<()> {
    let Some(to) = args.second else {
        let to = args.first;
        call(Call::RenameHere { to: to.clone() })?;
        println!("this computer is {to} now; the names it had still name it");
        return Ok(());
    };
    let machine = args.first;
    let account = account_or_owner(args.hub.as_ref(), args.account, Some(&machine))?;
    self::change(args.hub, account, AccountChange::Rename { machine: machine.clone(), to: to.clone() })?;
    println!("{machine} is {to} now; {machine} still names it");
    Ok(())
}

/// Groups machines, or renames or ends a group (`world-network.md`, 机器的分组).
pub fn group(args: GroupArgs) -> anyhow::Result<()> {
    if let Some(to) = args.rename {
        let [from] = args.machines.as_slice() else { anyhow::bail!("--rename takes the one group's name now") };
        let to = fastctx_types::accounts::group_name(&to).map_err(anyhow::Error::msg)?;
        let owner = account_or_owner(args.hub.as_ref(), args.account, None)?;
        self::change(
            args.hub,
            owner.clone(),
            AccountChange::RenameGroup { owner, from: from.clone(), to: to.clone() },
        )?;
        match to {
            Some(to) => println!("the group {from} is {to} now"),
            None => println!("the group {from} is no more; its machines are in no group"),
        }
        return Ok(());
    }
    let group = match (&args.to, args.none) {
        (Some(to), _) => Some(
            fastctx_types::accounts::group_name(to)
                .map_err(anyhow::Error::msg)?
                .ok_or_else(|| anyhow::anyhow!("a group has a name"))?,
        ),
        (None, true) => None,
        (None, false) => anyhow::bail!("say the group with --to, or --none to take them out of theirs"),
    };
    for machine in &args.machines {
        let machine: MachineName = machine.parse().map_err(|error| anyhow::anyhow!("{machine}: {error}"))?;
        let account = account_or_owner(args.hub.as_ref(), args.account.clone(), Some(&machine))?;
        self::change(
            args.hub.clone(),
            account,
            AccountChange::Group { machine: machine.clone(), group: group.clone() },
        )?;
        match &group {
            Some(group) => println!("{machine} is in the group {group}"),
            None => println!("{machine} is in no group"),
        }
    }
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
            // A hub dissolved here before still tells the machines that were
            // away; one whose service is replaced anyway is ended first.
            if service && fastctx_hub::dissolve::dissolved(&dir)?.is_some() {
                crate::hub_retire::retire_elevated(&dir)?;
            }
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
            // A hub holds a connection for every machine and transfer: as
            // many as the system lets it, and it goes on with whatever it has.
            if let Err(error) = fastctx_platform::limits::raise_open_files() {
                eprintln!("fastctx hub: the limit on open files stays as it was: {error}");
            }
            let leave = fastctx_hub::Leave { remove_service: Box::new(remove_hub_service) };
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()?
                .block_on(fastctx_hub::serve(&dir, leave))?;
        }
        HubCommand::Arrive { dir: chosen } => {
            let dir = dir(chosen);
            let Some((hub, id)) = fastctx_hub::arriving(&dir)? else {
                anyhow::bail!("no hub is moving to this computer in {}", dir.display());
            };
            if !keep_arriving_hub(&dir)? {
                anyhow::bail!("the hub's service is not running here, so the move waits");
            }
            println!("the hub's service runs here from {}", dir.display());
            match call(Call::HubArrived { hub })? {
                Reply::Done => println!("the old hub is told: it reaches this one and hands over (move {id})"),
                other => anyhow::bail!("the node answered something else: {other:?}"),
            }
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
            if let Some(dissolved) = &hub.dissolved {
                println!(
                    "dissolved by {} at {}; {} machines not told yet",
                    dissolved.by,
                    dissolved.at.strftime("%Y-%m-%d %H:%M:%S UTC"),
                    dissolved.untold.len()
                );
            }
        }
        HubCommand::Retire { dir: chosen } => {
            let (name, untold) = crate::hub_retire::retire(&dir(chosen))?;
            match untold {
                0 => println!("the hub of the dissolved network {name} is ended here"),
                untold => println!(
                    "the hub of the dissolved network {name} is ended here; {untold} machines that were away were not told"
                ),
            }
        }
    }
    Ok(())
}

/// Removes the system service of the hub kept in `dir`, which moved away
/// and whose old data is deleted. The service's removal stops this very
/// process, so another one does it, started on its own.
fn remove_hub_service(dir: &std::path::Path) -> Result<String, String> {
    use fastctx_install::service::{Kind, Service};
    let program = crate::hub_retire::program(dir).map_err(|error| format!("finding this program: {error}"))?;
    let service = Service::new(Kind::Hub, &program, dir);
    let status = service.status().map_err(|error| format!("looking for the hub's service: {error}"))?;
    if !status.installed {
        return Ok("no system service runs this hub here; there is none to remove".to_string());
    }
    let cwd = dir.parent().ok_or("the hub's directory has no parent")?;
    let args: Vec<&std::ffi::OsStr> = vec![
        "service".as_ref(),
        "hub".as_ref(),
        "uninstall".as_ref(),
        "--dir".as_ref(),
        dir.as_os_str(),
        "--executable".as_ref(),
        program.as_os_str(),
    ];
    let env: Vec<(std::ffi::OsString, std::ffi::OsString)> = std::env::vars_os().collect();
    let started = fastctx_platform::spawn_detached(&program, &args, cwd, &env)
        .map_err(|error| format!("starting the removal of the service {}: {error}", status.name))?;
    let pid = started.identity.map(|identity| identity.pid.to_string()).unwrap_or_else(|| "ended".to_string());
    // Still in the service's own job, the removal may end with the service it stops.
    let bound = if started.ends_with_starters_job { "; it could not leave the service's job" } else { "" };
    Ok(format!("removing the hub's system service {} (process {pid}){bound}", status.name))
}

/// Runs the hub moving to this computer, kept in `dir`, as a system service
/// from the World program this computer's node runs from; whether it runs
/// now.
fn keep_arriving_hub(dir: &std::path::Path) -> anyhow::Result<bool> {
    use fastctx_install::service::Kept;
    let places = fastctx_install::apply::Places::here()?;
    let installed = places.installed();
    if !installed.is_file() {
        return keep_hub(dir);
    }
    match fastctx_install::service::keep_hub_at(&places, dir, &installed)? {
        Kept::Installed | Kept::Already => Ok(true),
        Kept::Declined => anyhow::bail!("administrator rights were declined; the hub's service is not installed"),
        Kept::Command(command) => {
            eprintln!("Its service needs administrator rights: {command}");
            sudo_hub_at(dir, &installed)
        }
        Kept::Unsupported => {
            eprintln!("This system has no service for it.");
            Ok(false)
        }
    }
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
    let (executable, _) = fastctx_install::apply::place_executable(places)?;
    sudo_hub_at(dir, &executable)
}

/// Installs the service of the hub in `dir`, run from `executable`, through
/// sudo, as `sudo_hub` does.
#[cfg(unix)]
fn sudo_hub_at(dir: &std::path::Path, executable: &std::path::Path) -> anyhow::Result<bool> {
    use std::io::IsTerminal as _;
    let mut sudo = std::process::Command::new("sudo");
    if !std::io::stdin().is_terminal() {
        sudo.args(["-S", "-p", ""]);
    }
    let status = sudo
        .arg(executable)
        .args(["service", "hub", "install", "--dir"])
        .arg(dir)
        .arg("--executable")
        .arg(executable)
        .status()
        .map_err(|error| anyhow::anyhow!("starting sudo for the hub's service: {error}"))?;
    anyhow::ensure!(status.success(), "sudo service registration failed ({status}); run the command above to finish");
    Ok(true)
}

#[cfg(not(unix))]
fn sudo_hub(_: &fastctx_install::apply::Places, _: &std::path::Path) -> anyhow::Result<bool> {
    Ok(false)
}

#[cfg(not(unix))]
fn sudo_hub_at(_: &std::path::Path, _: &std::path::Path) -> anyhow::Result<bool> {
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
        && let Some(owner) = seen
            .members
            .iter()
            .find(|member| member.name == *machine || member.was.contains(machine))
            .and_then(|member| {
                member.home.clone().or_else(|| member.logins.iter().find(|login| seen.logins.contains(login)).cloned())
            })
        && seen.logins.contains(&owner)
        && seen.accounts.iter().any(|account| account.name == owner && !account.locked)
    {
        return Ok(owner);
    }
    if seen.logins.len() > 1
        && let Some(machine) = machine
    {
        // Use the node's account-policy result, just as the machine page does.
        // Resolve former names through Accounts before selecting the rights row.
        let name = seen
            .members
            .iter()
            .find(|member| member.name == *machine || member.was.contains(machine))
            .map_or(machine, |member| &member.name);
        if let Reply::Nodes(nodes) = call(Call::Nodes)?
            && let Some(hub) = nodes.hubs.iter().find(|hub| hub.hub == seen.hub)
        {
            let rights = if seen.me.as_ref() == Some(name) {
                Some(&hub.here)
            } else {
                hub.machines.iter().find(|member| member.name == *name).map(|member| &member.rights)
            };
            let managers: Vec<_> = rights
                .into_iter()
                .flatten()
                .filter(|(account, rights)| rights.hands && seen.logins.contains(account))
                .map(|(account, _)| account)
                .collect();
            if let [only] = managers.as_slice() {
                return Ok((*only).clone());
            }
        }
    }
    match seen.logins.as_slice() {
        [only] => Ok(only.clone()),
        [] => anyhow::bail!("this computer is logged in to no account on {}", seen.name),
        _ => anyhow::bail!("this computer is logged in to several accounts on {}; name one with --as", seen.name),
    }
}

/// How long a code stays valid: hours or days, as `1h` or `7d`, up to a
/// year; or `never`.
fn valid(text: &str) -> Result<Valid, String> {
    if text == "never" {
        return Ok(Valid::Never);
    }
    let said = "give hours or days, as 1h or 7d, up to a year, or never";
    let (count, unit) = text.split_at(text.len().saturating_sub(1));
    let count: i64 = count.parse().map_err(|_| said.to_string())?;
    let hours = match unit {
        "h" => count,
        "d" => count.saturating_mul(24),
        _ => return Err(said.to_string()),
    };
    if !(1..=366 * 24).contains(&hours) {
        return Err(said.to_string());
    }
    Ok(Valid::For(SignedDuration::from_hours(hours)))
}

/// How many devices a code lets in: a number up to 9999, or `any`, said
/// as zero.
fn uses(text: &str) -> Result<u32, String> {
    if text == "any" {
        return Ok(0);
    }
    match text.parse::<u32>() {
        Ok(uses @ 1..=9999) => Ok(uses),
        _ => Err("give a number from 1 to 9999, or any".to_string()),
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
            LinkState::Stopped { reason, dissolved: true } => reason.clone(),
            LinkState::Stopped { reason, .. } => format!("the hub removed this computer: {reason}"),
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
        LinkState::Stopped { reason, dissolved: true } => reason.clone(),
        LinkState::Stopped { reason, .. } => format!("the hub removed this computer: {reason}"),
    }
}

fn render_nodes(reply: &NodesReply, now: Timestamp) -> String {
    // Without its facts, this computer is not one the asker may work on,
    // and goes unnamed.
    let mut text = match reply.facts {
        Some(_) => format!("=== nodes: {} ===\n", reply.machine),
        None => "=== nodes ===\n".to_string(),
    };
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
    // A managed machine works for no account: agents elsewhere run
    // commands on it, and it reaches no other machine.
    if let (true, Some(home)) = (hub.logins.is_empty(), &hub.home) {
        return format!(
            "\n--- hub {} ({}) · a managed machine of {home}: agents on other machines work here, and this computer reaches no other machine\n",
            hub.name,
            link(&hub.link, now)
        );
    }
    let home = hub.home.as_ref().map(|home| format!("; registered under {home}")).unwrap_or_default();
    // A session's are the accounts it works for, logged in here or not.
    let accounts = if resident { format!("working for {logins}") } else { format!("logged in to {logins}") };
    let mut text = format!("\n--- hub {} ({}) · {accounts}{home}\n", hub.name, link(&hub.link, now));
    if hub.machines.is_empty() {
        text.push_str(&format!("no other machine is in {}: this computer is the only machine\n", hub.name));
    }
    // Names line up by the columns they take in a terminal: a Chinese
    // character takes two.
    let width = hub.machines.iter().map(|node| fastctx_tui::text::width(&node.written.to_string())).max().unwrap_or(0);
    // Machines in no group first, then each group under its name: a group
    // only shows its machines together (`world-network.md`, 机器的分组).
    let mut shown: Vec<&fastctx_types::world::Node> = hub.machines.iter().filter(|node| node.group.is_none()).collect();
    let mut groups: Vec<(Option<&AccountName>, &str)> = Vec::new();
    for node in &hub.machines {
        if let Some(group) = &node.group
            && !groups.contains(&(node.owner.as_ref(), group.as_str()))
        {
            groups.push((node.owner.as_ref(), group.as_str()));
        }
    }
    groups.sort();
    let mut headers: Vec<(usize, String)> = Vec::new();
    for (owner, group) in &groups {
        let whose =
            owner.filter(|owner| !hub.logins.contains(owner)).map(|owner| format!("{owner}'s ")).unwrap_or_default();
        headers.push((shown.len(), format!("group {whose}{group}:\n")));
        shown.extend(
            hub.machines.iter().filter(|node| node.owner.as_ref() == *owner && node.group.as_deref() == Some(*group)),
        );
    }
    for (at, node) in shown.into_iter().enumerate() {
        for (_, header) in headers.iter().filter(|(starts, _)| *starts == at) {
            text.push_str(header);
        }
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
        // Missing facts are explained only for a machine known to be online:
        // of the others, being offline or on a hub not reached now says why.
        let facts = match &node.facts {
            Ok(facts) => format!("  {}", facts_line(facts)),
            Err(_) if node.online != Some(true) => String::new(),
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
    for dissolution in &reply.dissolved {
        text.push_str(&format!(
            "the network {} was dissolved by its administrator {} ago; this computer left it, and kept all it has\n",
            dissolution.hub,
            age(dissolution.at, now)
        ));
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
            match account.level {
                fastctx_types::accounts::Level::Super => marks.push("super administrator"),
                fastctx_types::accounts::Level::Visitor => marks.push("visitor"),
                fastctx_types::accounts::Level::Member => {}
            }
            if account.promotion {
                marks.push("asked to become a member");
            }
            if account.locked {
                marks.push("locked");
            }
            let marks = if marks.is_empty() { String::new() } else { format!(" ({})", marks.join(", ")) };
            text.push_str(&format!("account {}{marks}\n", account.name));
            for member in hub.members.iter().filter(|member| member.home.as_ref() == Some(&account.name)) {
                let managed = if member.managed { " (managed)" } else { "" };
                let group = member.group.as_ref().map(|group| format!(" in the group {group}")).unwrap_or_default();
                text.push_str(&format!("  machine {} [{}]{managed}{group}\n", member.name, member.key));
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
        for assist in &hub.assists {
            let login =
                assist.home.as_ref().map(|home| home.display().to_string()).unwrap_or_else(|| "default".to_string());
            let until = assist.until.map(|until| format!(" until {}", local(until))).unwrap_or_default();
            text.push_str(&format!(
                "the {} login {login} on {} assists {}{until}\n",
                assist.agent, assist.machine, assist.account
            ));
        }
        for acting in &hub.acting {
            text.push_str(&format!(
                "the agents on {} reach {}'s machines, for {}\n",
                acting.machine,
                acting.account,
                age(acting.since, now)
            ));
        }
        for token in &hub.tokens {
            text.push_str(&format!("code {}\n", token_line(token, now)));
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
