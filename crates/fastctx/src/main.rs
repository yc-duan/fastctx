//! The `fastctx` command.

use std::io::Write as _;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

#[cfg(all(target_os = "linux", target_env = "musl"))]
mod allocator;
mod apply;
mod ask;
mod attach;
mod claude_session_directory;
mod claude_subagents;
mod command_guard;
mod copy;
mod data;
mod doctor;
mod first_program;
mod host_change;
mod hub_intake;
mod hub_retire;

mod keep;
mod mcp;
mod mcp_child;
mod mcp_relay;
mod on;
mod output;
mod page;
mod paths;
mod plan;
mod profile;
mod program;
mod project;
mod proxy;
mod records;
mod renew;
mod repair;
mod secret;
mod self_update;
mod sense;
mod skill;
mod server_install;
mod service;
#[cfg(windows)]
mod service_local_network;
mod steps;
mod stop;
mod talk;
mod timer;
mod goal;
mod turn_facts;
mod uninstall;
mod update;
mod view;
mod w;
mod wait;
mod world;
mod writers;

/// The version `--version` prints; a development build, which keeps its
/// World apart (`fastctx_platform::RELEASED`), says it is one.
const VERSION: &str = if fastctx_platform::RELEASED {
    env!("CARGO_PKG_VERSION")
} else {
    concat!(env!("CARGO_PKG_VERSION"), " (development build)")
};

#[derive(Parser)]
#[command(name = "fastctx", bin_name = "fastctx", version = VERSION, about = "One world, many machines.")]
struct Cli {
    /// Print this executable's licenses and third-party notices.
    #[arg(long, exclusive = true)]
    licenses: bool,
    /// Without a command, in a terminal, the TUI opens.
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Inspect this node's current work.
    #[command(subcommand)]
    Doctor(doctor::Doctor),
    /// The account's MCP servers: list, add, import, test and remove.
    Mcp(mcp::McpArgs),
    /// Read or change the account's instructions and host settings.
    Profile(profile::ProfileArgs),
    #[command(about = fastctx_types::plan::words::HELP)]
    Plan(plan::PlanArgs),
    #[command(hide = true)]
    Records(records::RecordsArgs),
    /// Leave a sentinel to wait for something.
    Sense(sense::SenseArgs),
    /// Read one of the account's skills, by its name.
    Skill(skill::SkillArgs),
    #[command(hide = true)]
    McpRelay { name: String },
    #[command(hide = true)]
    McpChild {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<std::ffi::OsString>,
    },
    /// Open the World in this terminal: projects, agents, machines, and a
    /// conversation with any agent on any machine.
    Ui,
    #[command(hide = true)]
    Timer(timer::TimerArgs),
    /// Read or mark this conversation's goal.
    Goal(goal::GoalArgs),
    #[command(hide = true)]
    TurnFacts(turn_facts::TurnFactsArgs),
    #[command(hide = true)]
    HostChange(host_change::HostChangeArgs),
    /// Identifies the process invoking the World mod helper.
    #[command(hide = true)]
    ParentProcess,
    #[command(name = "command-guard", hide = true)]
    Guard(command_guard::Args),
    #[command(hide = true)]
    ClaudeSubagents {
        #[arg(value_parser = paths::parser())]
        world: PathBuf,
        session: String,
    },
    #[command(hide = true)]
    ClaudeSessionDirectory {
        #[arg(value_parser = paths::parser())]
        world: PathBuf,
        session: String,
        #[arg(long)]
        previous: Option<String>,
        #[arg(long)]
        cwd: Option<String>,
        #[arg(long)]
        conversation: Option<String>,
    },
    /// Wait for the person answering one public Claude mod request.
    #[command(hide = true)]
    ClaudeRequest {
        #[arg(value_parser = paths::parser())]
        directory: PathBuf,
    },
    /// The official plugin's MCP elicitation command hook.
    #[command(hide = true)]
    ClaudeElicitation {
        #[arg(value_parser = paths::parser())]
        world: PathBuf,
    },
    /// Claims a queued control once, across Claude plugin activations.
    #[command(hide = true)]
    ClaudeControlClaim {
        #[arg(value_parser = paths::parser())]
        world: PathBuf,
    },
    /// Observes the native goal cleanup before controls consume its context.
    #[command(hide = true)]
    ClaudeNativeGoal {
        #[arg(value_parser = paths::parser())]
        world: PathBuf,
        session: String,
        #[arg(long)]
        after: Option<u64>,
    },
    /// Identifies the node lifetime that may issue queued Claude controls.
    #[command(hide = true)]
    ClaudeControlOwner {
        #[arg(value_parser = paths::parser())]
        world: PathBuf,
    },
    /// A lifetime witness for a Claude mod activation, and the line that
    /// wakes it when the node leaves its session something.
    #[command(hide = true)]
    ClaudeControlGuard {
        /// The session's directory, watched for what the node leaves there.
        #[arg(long, value_parser = paths::parser(), requires = "root")]
        watch: Option<PathBuf>,
        /// fastctx's root, where a removal leaves its marker.
        #[arg(long, value_parser = paths::parser())]
        root: Option<PathBuf>,
    },
    #[command(hide = true)]
    ClaudeSessionPolicy {
        #[arg(value_parser = paths::parser())]
        world: PathBuf,
        session: String,
        #[arg(long)]
        messages: Option<usize>,
        #[arg(long)]
        after_compact: Option<String>,
    },
    /// Run this machine's node in the foreground.
    Node {
        /// The fastctx root to serve; defaults to this user's fastctx root.
        #[arg(long, value_parser = paths::parser())]
        dir: Option<PathBuf>,
        /// A guardian keeps this node; with the two below, the channel it
        /// handed down (`fastctx_node::handoff`).
        #[arg(long, hide = true, requires_all = ["supervisor_control", "supervisor_events", "service_identity"])]
        supervised: bool,
        #[arg(long, hide = true)]
        supervisor_control: Option<u64>,
        #[arg(long, hide = true)]
        supervisor_events: Option<u64>,
        #[arg(long, hide = true, value_parser = ["s4u", "interactive", "system"])]
        service_identity: Option<String>,
        #[arg(long, hide = true)]
        successor: Option<String>,
        #[arg(long, hide = true, requires = "successor")]
        prepared_by: Option<String>,
    },
    /// Replace this machine's node and guardian using a local executable.
    Renew(renew::RenewalArgs),
    /// Install, uninstall or inspect a node/hub service on this machine.
    Service(service::ServiceArgs),
    #[cfg(windows)]
    #[command(hide = true)]
    ServiceLocalNetwork(service_local_network::LocalNetworkArgs),
    /// Preview or execute removal of the World installed on this computer.
    #[command(subcommand)]
    Uninstall(uninstall::Uninstall),
    /// Install a verified release on a Linux server and join a World.
    #[command(subcommand)]
    Server(server_install::ServerCommand),
    /// The SYSTEM guardian, dispatched by the Windows service controller.
    #[cfg(windows)]
    #[command(hide = true)]
    ServiceHost { name: String },
    /// An independent check of the SYSTEM guardian.
    #[cfg(windows)]
    #[command(hide = true)]
    ServiceCheck { name: String },
    /// Serve the World's tools to an agent session over MCP's stdio.
    Serve {
        /// Accepted from configurations written when files were written with
        /// tools of their own; files are written in the World's shell now.
        #[arg(long, hide = true)]
        writer: Option<String>,
        /// `off` turns the World's write commands off in this session's
        /// shell: they refuse and point at the host's own.
        #[arg(long, default_value = "on", value_parser = ["on", "off"])]
        writes: String,
    },
    /// Look at files: text with line numbers, images as images, a PDF's text
    /// by page or its pages as images, Word, PowerPoint and Excel content.
    View(view::ViewArgs),
    /// Edit a file using SEARCH/REPLACE blocks from standard input.
    Edit(writers::EditArgs),
    /// Create or replace a file with the text from standard input.
    Write(writers::WriteArgs),
    /// Apply a patch from standard input, written as Codex's apply_patch
    /// writes one: `*** Begin Patch` … `*** End Patch`; paths are absolute.
    #[command(hide = true)]
    ApplyPatch,
    /// See what runs and who works now, and what happened since your last
    /// turn; with ids, those sessions or jobs.
    W(w::WArgs),
    /// Run part of a script on another of the user's machines.
    #[command(override_usage = on::USAGE, after_help = on::AFTER_HELP)]
    On(on::OnArgs),
    /// Stop jobs and everything they started, on whichever machine each runs.
    Stop(stop::StopArgs),
    /// Wait until jobs end, then show how each ended and its last lines.
    ///
    /// The jobs are those of this machine; for a job on another machine, wait
    /// there: `on laptop fastctx wait j-k3x9a2`. The end of a job you started
    /// is shown here and not told to you again. Like bash's `wait`, it exits
    /// with the status of the last job named.
    Wait(wait::WaitArgs),
    /// List the user's machines: which are online and what each is like.
    Nodes,
    /// Copy files and directories on one of the user's machines or between them.
    ///
    /// Each end is MACHINE:PATH, or a path on this machine; the last is the
    /// destination. The destination path is what the copy becomes; ending it
    /// with / copies into that directory instead, as several sources must. Files already there with the same contents are not
    /// sent again, so running the same copy again sends only what changed.
    /// Links are copied as links. The bytes never go through the terminal.
    Copy(copy::CopyArgs),
    /// Talk to a resident on any machine, or open a session to talk to.
    Talk(talk::TalkArgs),
    /// Open a resident of this machine in its agent's own interface, in
    /// this terminal.
    Attach(attach::AttachArgs),
    /// The user's projects: list, show and enter them; open one or write
    /// the next version of its record.
    ///
    /// A fastctx project is not a folder or a repository: it is a named
    /// piece of the user's work that can span several machines. Its record
    /// says what it is for, which machines it spans, and where its code and
    /// data are on each; its pages show live facts to the user.
    #[command(subcommand)]
    Project(project::ProjectCommand),

    /// A project's pages: HTML the user opens in the World app, showing
    /// live facts of the machines and sessions. Publish, read or list them.
    #[command(subcommand, after_long_help = page::PAGE_HELP)]
    Page(page::PageCommand),
    /// Log this computer in to an account on a hub: with the hub's address
    /// and the account's password, asked for here, or with a token another
    /// device made. It stays logged in until it logs out or is removed.
    Login(world::LoginArgs),
    /// Log this computer out of an account.
    Logout(world::LogoutArgs),
    /// Codes that let other devices into an account: make one, for a
    /// server the line that installs and registers it; list those that
    /// still let devices in; or revoke one.
    #[command(subcommand)]
    Token(world::TokenCommand),
    /// The hubs this computer is on, with their accounts, machines, what is
    /// shared, which agents assist whom, and projects, and who came into its
    /// accounts.
    Accounts,
    /// Share one of an account's machines with another account: it may work
    /// on it and use every agent on it. Or take it back.
    Share(world::ShareArgs),
    /// Let one agent's login on one of an account's machines assist another
    /// account: that login works for it, the machine is not given. Or end
    /// the assistance.
    Assist(world::AssistArgs),
    /// The agents on a machine of another account, which this account used,
    /// no longer reach this account's machines.
    Disallow(world::DisallowArgs),
    /// Take a device or a machine out of an account.
    Remove(world::RemoveArgs),
    /// Dissolve a network, as its super administrator, typing its name:
    /// every machine and device is taken off it, and nobody comes in again.
    /// Fastctx stays on each computer, with its conversations and files.
    Dissolve(world::DissolveArgs),
    /// Rename a machine: this computer, one of an account's machines, or,
    /// for the network's administrator, any. Its names before still name it.
    Rename(world::RenameArgs),
    /// The HTTP proxy this computer reaches its hubs through: set it, clear
    /// it, or see it. It is kept in Fastctx, so the node its boot service
    /// starts uses it too.
    #[command(subcommand)]
    Proxy(proxy::ProxyCommand),
    /// Put machines in a group, or take them out: a group only shows them
    /// together. Or rename a group, or end it.
    Group(world::GroupArgs),
    /// Let other machines connect to a machine directly, at addresses you
    /// choose, instead of through the hub; or no longer. The network's
    /// administrator's change.
    Direct(world::DirectArgs),
    /// The hub: where the accounts are, and every computer connects to.
    #[command(subcommand)]
    Hub(world::HubCommand),
    /// What fastctx keeps on this machine, conversation by conversation,
    /// and deleting it; how long command output is kept.
    Data(data::DataArgs),
    /// Bring this machine's World program up to the newest release on its
    /// channel, from GitHub or through the network; set how it updates.
    Update(self_update::UpdateArgs),
    /// Give the sessions you open on this machine, in Codex and Claude Code,
    /// the World's tools.
    Apply(apply::ApplyArgs),
    /// Take the World's tools out of the sessions you open on this machine.
    Unapply(apply::UnapplyArgs),
    /// Supervise one job; started by the node, not by people.
    #[command(hide = true)]
    Supervise {
        #[arg(value_parser = paths::parser())]
        world: PathBuf,
        #[arg(value_parser = paths::parser())]
        record: PathBuf,
    },
}

fn main() -> ExitCode {
    // Image workers must set their memory limit before starting a runtime or
    // entering the World; failures remain isolated from the serving node.
    if std::env::args_os().nth(1).is_some_and(|argument| argument == "thumbnail-worker") {
        let arguments: Vec<_> = std::env::args_os().skip(2).collect();
        let result = (|| {
            anyhow::ensure!(arguments.len() == 3, "thumbnail-worker needs path, max and allowance");
            let max = arguments[1].to_str().ok_or_else(|| anyhow::anyhow!("invalid max"))?.parse()?;
            let budget = arguments[2].to_str().ok_or_else(|| anyhow::anyhow!("invalid allowance"))?.parse()?;
            fastctx_node::thumbnail::worker(PathBuf::from(&arguments[0]).as_path(), max, budget)
                .map_err(anyhow::Error::msg)
        })();
        return finish(result.map(|()| ExitCode::SUCCESS));
    }
    // A command forwarded to the World program ends with the entry it came
    // through, and the entry's parent is the process that started it; what
    // it starts does not inherit that.
    #[cfg(windows)]
    if let Some(forwarder) = std::env::var_os(program::FORWARDER) {
        // SAFETY: the first thing in main, before any other thread exists.
        unsafe { std::env::remove_var(program::FORWARDER) };
        if let Some(pid) = forwarder.to_str().and_then(|text| text.parse::<u32>().ok()) {
            fastctx_platform::host_process::forwarded_by(pid);
            if let Err(error) = fastctx_platform::parent::follow_forwarder(pid) {
                eprintln!("fastctx: {error}");
                return ExitCode::FAILURE;
            }
        }
    }
    if std::env::args_os().skip(1).eq(["--licenses"]) {
        return print_licenses();
    }
    // An old fastctx's upgrader may have started this version: it waits to
    // hear that it is healthy before anything else happens.
    if std::env::args_os().nth(1).is_some_and(|arg| arg == "mcp-child") {
        let args = std::env::args_os().skip(2).skip_while(|arg| arg == "--").collect();
        return finish(mcp_child::run(args).map(|()| ExitCode::SUCCESS));
    }
    let readonly_uninstall = std::env::args_os().skip(1).take(2).eq(["uninstall", "plan"]);
    if !readonly_uninstall && let Err(error) = update::finalize() {
        eprintln!("fastctx: {error:#}");
        return ExitCode::FAILURE;
    }
    let cli = match <Cli as clap::FromArgMatches>::from_arg_matches(&command_line().get_matches()) {
        Ok(cli) => cli,
        Err(error) => error.exit(),
    };
    if cli.licenses {
        return print_licenses();
    }
    let Some(command) = cli.command else {
        // The TUI needs a terminal on both sides; anything else asked for
        // help, as a command without arguments does.
        use std::io::IsTerminal as _;
        if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
            return finish(ui().map(|()| ExitCode::SUCCESS));
        }
        let _ = command_line().print_help();
        return ExitCode::from(2);
    };
    let routes_world = !matches!(
        &command,
        Command::Ui
            | Command::HostChange(_)
            | Command::Node { .. }
            | Command::Renew(_)
            | Command::Service(_)
            | Command::Server(_)
            | Command::Hub(world::HubCommand::Serve { .. })
            | Command::Mcp(_)
            | Command::Profile(_)
            | Command::Plan(_)
            | Command::Skill(_)
            | Command::Records(_)
            | Command::McpRelay { .. }
            | Command::McpChild { .. }
            | Command::Serve { .. }
            | Command::Apply(_)
            | Command::Unapply(_)
            | Command::Supervise { .. }
            | Command::ParentProcess
            | Command::Guard(_)
            | Command::ClaudeSessionDirectory { .. }
            | Command::ClaudeSubagents { .. }
            | Command::ClaudeRequest { .. }
            | Command::ClaudeElicitation { .. }
            | Command::ClaudeControlOwner { .. }
            | Command::ClaudeControlClaim { .. }
            | Command::ClaudeNativeGoal { .. }
            | Command::ClaudeControlGuard { .. }
            | Command::ClaudeSessionPolicy { .. }
            | Command::Uninstall(_)
    );
    #[cfg(windows)]
    let routes_world = routes_world
        && !matches!(
            &command,
            Command::ServiceHost { .. } | Command::ServiceCheck { .. } | Command::ServiceLocalNetwork(_)
        );
    if matches!(command, Command::Uninstall(_)) {
        let routed =
            fastctx_platform::fastctx_root().map_err(anyhow::Error::from).and_then(|root| program::removal(&root));
        match routed {
            Ok(Some(result)) => return finish(result.map(|()| ExitCode::SUCCESS)),
            Err(error) => return finish(Err(error)),
            Ok(None) => (),
        }
    }
    if routes_world {
        let routed =
            fastctx_platform::fastctx_root().map_err(anyhow::Error::from).and_then(|root| program::world(&root));
        match routed {
            Ok(Some(result)) => return finish(result.map(|()| ExitCode::SUCCESS)),
            Err(error) => return finish(Err(error)),
            Ok(None) => (),
        }
    }
    let result = match command {
        Command::Ui => ui(),
        Command::Doctor(command) => doctor::run(command),
        Command::Timer(args) => timer::run(args),
        Command::Goal(args) => goal::run(args),
        Command::TurnFacts(args) => turn_facts::run(args),
        Command::HostChange(args) => host_change::run(args),
        Command::ParentProcess => parent_process(),
        Command::Guard(args) => command_guard::run(args),
        Command::ClaudeSubagents { world, session } => claude_subagents::run(&world, session),
        Command::ClaudeSessionDirectory { world, session, previous, cwd, conversation } => {
            claude_session_directory::run(&world, session, previous, cwd, conversation)
        }
        Command::ClaudeRequest { directory } => {
            fastctx_node::claude_request::run(&directory, false).map_err(Into::into)
        }
        Command::ClaudeElicitation { world } => fastctx_node::claude_request::run(&world, true).map_err(Into::into),
        Command::ClaudeControlOwner { world } => fastctx_node::claude_request::owner(&world).map_err(Into::into),
        Command::ClaudeControlClaim { world } => fastctx_node::claude_control_claim::run(&world).map_err(Into::into),
        Command::ClaudeNativeGoal { world, session, after } => {
            fastctx_node::claude_native_goal::run(&world, &session, after).map_err(Into::into)
        }
        Command::ClaudeControlGuard { watch, root } => {
            fastctx_node::claude_request::guard(watch.as_deref().zip(root.as_deref())).map_err(Into::into)
        }
        Command::ClaudeSessionPolicy { world, session, messages, after_compact } => {
            fastctx_node::claude_request::policy(&world, &session, messages, after_compact.as_deref())
                .map_err(Into::into)
        }
        Command::Node {
            dir,
            supervised,
            supervisor_control,
            supervisor_events,
            service_identity,
            successor,
            prepared_by,
        } => {
            let supervision = match (supervised, supervisor_control, supervisor_events) {
                (true, Some(control), Some(events)) => Some((control, events, service_identity)),
                _ => None,
            };
            node(dir, supervision, successor, prepared_by)
        }
        Command::Renew(args) => renew::run(args),
        Command::Service(args) => service::run(args),
        #[cfg(windows)]
        Command::ServiceLocalNetwork(args) => service_local_network::run(args),
        Command::Uninstall(command) => uninstall::run(command),
        Command::Server(command) => server_install::run(command),
        #[cfg(windows)]
        Command::ServiceHost { name } => fastctx_install::service::run_host(&name),
        #[cfg(windows)]
        Command::ServiceCheck { name } => fastctx_install::service::check_guardian(&name),
        Command::Mcp(args) => mcp::run(args),
        Command::Profile(args) => profile::run(args),
        Command::Plan(args) => plan::run(args),
        Command::Records(args) => records::run(args),
        Command::Sense(args) => sense::run(args),
        Command::Skill(args) => skill::run(args),
        Command::McpRelay { name } => mcp_relay::run(name),
        Command::McpChild { args } => mcp_child::run(args),
        Command::Serve { writer: _, writes } => serve(writes == "off"),
        Command::View(args) => return told(finish(view::run(args))),
        Command::Edit(args) => return told(finish(writers::edit(args))),
        Command::Write(args) => return told(finish(writers::write(args))),
        Command::ApplyPatch => return told(finish(writers::apply_patch())),
        Command::W(args) => w::run(args),
        Command::On(args) => return on::run(args),
        Command::Stop(args) => return finish(stop::run(args)),
        Command::Wait(args) => return finish(wait::run(args)),
        Command::Nodes => world::nodes(),
        Command::Copy(args) => return told(finish(copy::run(args))),
        Command::Talk(args) => talk::run(args),
        Command::Attach(args) => attach::run(args),
        Command::Project(command) => project::run(command),

        Command::Page(command) => page::run(command),
        Command::Login(args) => world::login(args),
        Command::Logout(args) => world::logout(args),
        Command::Token(command) => world::token(command),
        Command::Accounts => world::accounts(),
        Command::Data(args) => data::run(args),
        Command::Update(args) => self_update::run(args),
        Command::Share(args) => world::share(args),
        Command::Assist(args) => world::assist(args),
        Command::Disallow(args) => world::disallow(args),
        Command::Remove(args) => world::remove(args),
        Command::Dissolve(args) => world::dissolve(args),
        Command::Rename(args) => world::rename(args),
        Command::Proxy(command) => proxy::run(command),
        Command::Group(args) => world::group(args),
        Command::Direct(args) => world::direct(args),
        Command::Hub(command) => world::hub(command),
        Command::Apply(args) => apply::apply(args),
        Command::Unapply(args) => apply::unapply(args),
        Command::Supervise { world, record } => supervise(&world, &record),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("fastctx: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn print_licenses() -> ExitCode {
    finish(fastctx_ripgrep::licenses::write(output::out()).map(|()| ExitCode::SUCCESS).map_err(Into::into))
}

/// The commands a person runs on a machine: its node, joining and managing
/// the World, talking to residents, the TUI.
const PEOPLE_ONLY: [&str; 24] = [
    "ui",
    "data",
    "node",
    "renew",
    "service",
    "server",
    "serve",
    "talk",
    "attach",
    "login",
    "logout",
    "token",
    "accounts",
    "share",
    "assist",
    "disallow",
    "remove",
    "rename",
    "proxy",
    "direct",
    "hub",
    "apply",
    "unapply",
    "uninstall",
];

/// The command line; in a resident's shell its help lists what a resident
/// works with, the others staying callable.
fn command_line() -> clap::Command {
    let mut command = <Cli as clap::CommandFactory>::command();
    if std::env::var_os("FASTCTX_RESIDENT").is_some() {
        command =
            command.about("Commands for working on this machine, and on the user's other machines when there are any.");
        for name in PEOPLE_ONLY {
            command = command.mut_subcommand(name, |subcommand| subcommand.hide(true));
        }
    }
    command
}

/// A command that sets its own exit code, or fails.
fn finish(result: anyhow::Result<ExitCode>) -> ExitCode {
    result.unwrap_or_else(|error| {
        if error.is::<paths::Said>() {
            eprintln!("{error}");
        } else {
            eprintln!("fastctx: {error:#}");
        }
        ExitCode::FAILURE
    })
}

/// One of the World's file commands, inside a `run`: when it fails, the
/// run's answer names it as it was given, whatever the script's exit status
/// (`world-run.md`).
fn told(code: ExitCode) -> ExitCode {
    if code == ExitCode::SUCCESS || std::env::var_os("FASTCTX_MACHINE").is_none() {
        return code;
    }
    let mut args = std::env::args().skip(1);
    let mut given = match args.next().as_deref() {
        Some(name @ ("edit" | "write")) => name.to_string(),
        Some("apply-patch") => "apply_patch".to_string(),
        Some(name) => format!("fastctx {name}"),
        None => return code,
    };
    for arg in args {
        let arg: String = arg.chars().filter(|character| !character.is_control()).collect();
        if arg.is_empty() || arg.contains([' ', '\'', '"']) {
            given.push_str(&format!(" '{}'", arg.replace('\'', "'\\''")));
        } else {
            given.push(' ');
            given.push_str(&arg);
        }
    }
    let mark = fastctx_hands::run::terminal::FAILED_MARK;
    let mut out = std::io::stdout();
    let _ = std::io::Write::write_all(&mut out, format!("{mark}{given}\x07").as_bytes());
    let _ = std::io::Write::flush(&mut out);
    code
}

fn node(
    dir: Option<PathBuf>,
    supervision: Option<(u64, u64, Option<String>)>,
    successor: Option<String>,
    prepared_by: Option<String>,
) -> anyhow::Result<()> {
    // A node SYSTEM runs lives in the home Fastctx gives SYSTEM, and so does
    // everything it starts.
    fastctx_platform::system_home::prepare()?;
    // As many files as the system lets it hold: a lower limit only leaves
    // the node short sooner, and it goes on with whatever it has.
    if let Err(error) = fastctx_platform::limits::raise_open_files() {
        eprintln!("fastctx node: the limit on open files stays as it was: {error}");
    }
    let root = match dir {
        Some(root) => root,
        None => fastctx_platform::fastctx_root()?,
    };
    // What the node starts belongs to its World: a service starts it with
    // `--dir` alone, and its supervisors, the commands its jobs run and its
    // agents' sessions find their World by FASTCTX_HOME.
    if fastctx_platform::fastctx_root()? != root {
        // SAFETY: no other thread reads the environment before the node's
        // runtime starts below.
        unsafe { std::env::set_var("FASTCTX_HOME", &root) };
    }
    anyhow::ensure!(
        !root.join("uninstalled.json").try_exists()?,
        "this World was uninstalled; reinstall it before starting its node"
    );
    let coordinator =
        prepared_by.map(|text| serde_json::from_str::<fastctx_types::ProcessIdentity>(&text)).transpose()?;
    let startup_gate = if coordinator.is_none() {
        let (forwarded, gate) = world_entry(&root, true)?;
        if let Some(result) = forwarded {
            return result;
        }
        gate
    } else {
        None
    };
    // Recovery must also start when a newly installed node fails before it
    // can serve. A prepared successor is already watched by that worker.
    if successor.is_none() {
        match fastctx_install::renewal::read(&root) {
            Ok(Some(plan)) if plan.phase != fastctx_types::renewal::Phase::Complete => {
                if let Err(error) = fastctx_install::renewal::request(&root, false) {
                    eprintln!("fastctx: could not resume executable renewal: {error:#}");
                }
            }
            Err(error) => eprintln!("fastctx: could not read executable renewal: {error}"),
            _ => {}
        }
    }
    let supervision = match supervision {
        Some((control, events, identity)) => {
            let (control, events) = fastctx_platform::service_channel::inherited(control, events)?;
            let identity = match identity.as_deref() {
                Some("s4u") => fastctx_node::handoff::Identity::S4u,
                Some("system") => fastctx_node::handoff::Identity::System,
                _ => fastctx_node::handoff::Identity::Interactive,
            };
            Some(fastctx_node::handoff::Supervision { control, events, identity })
        }
        None => None,
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .thread_stack_size(fastctx_node::THREAD_STACK)
        .enable_all()
        .build()?;
    let ran = runtime.block_on(fastctx_node::run_prepared(
        &root,
        supervision,
        successor.as_deref(),
        coordinator.as_ref(),
        startup_gate,
    ));
    // A service manager starts the node again at once, and the World may be
    // held a while yet, by a renewal's bridge among others: a moment's pause,
    // once every lock is let go, keeps that from spinning.
    if let Err(error) = &ran
        && error.downcast_ref::<fastctx_node::AlreadyServed>().is_some()
    {
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
    ran
}

fn ui() -> anyhow::Result<()> {
    let root = fastctx_platform::fastctx_root()?;
    if let Some(result) = program::world(&root)? {
        return result;
    }
    fastctx_tui::run()
}

/// One per session and mostly waiting on the node, so one thread: a
/// machine with many sessions open should not hold a pool of threads per
/// core for each.
fn serve(writes_off: bool) -> anyhow::Result<()> {
    let root = fastctx_platform::fastctx_root()?;
    let (forwarded, gate) = world_entry(&root, false)?;
    if let Some(result) = forwarded {
        return result;
    }
    let world = fastctx_types::layout::WorldDir::under(&root);
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(fastctx_serve::serve_starting(world, writes_off, gate))
}

/// Delivery executables forward to the World program. The startup lock also
/// makes a process mapped just before publication re-check the installed build.
type Entry = (Option<anyhow::Result<()>>, Option<fastctx_platform::FileLock>);

fn world_entry(root: &std::path::Path, node: bool) -> anyhow::Result<Entry> {
    let source = std::env::current_exe()?;
    let installed = fastctx_install::executable::installed_path(root);
    let source_is_installed = installed.is_file() && stable_name(&source, &installed)?;
    first_program::ensure(root, &source)?;
    let gate = if node {
        fastctx_install::placement::node_activation(root)?
    } else {
        fastctx_install::placement::activation(root)?
    };
    let installed_build = fastctx_install::program::inspect(&installed)?;
    let own = fastctx_install::renewal::build()?;
    if source_is_installed && own == installed_build {
        return Ok((None, Some(gate)));
    }
    drop(gate);
    Ok((Some(program::forward(root, &installed)?), None))
}

fn stable_name(source: &std::path::Path, installed: &std::path::Path) -> std::io::Result<bool> {
    let installed = std::fs::canonicalize(installed)?;
    match std::fs::canonicalize(source) {
        Ok(source) => Ok(source == installed),
        Err(error) => {
            #[cfg(target_os = "linux")]
            if error.kind() == std::io::ErrorKind::NotFound {
                use std::os::unix::ffi::OsStrExt as _;
                if let Some(previous) = source.as_os_str().as_bytes().strip_suffix(b" (deleted)") {
                    return Ok(std::path::Path::new(std::ffi::OsStr::from_bytes(previous)) == installed);
                }
            }
            Err(error)
        }
    }
}

/// A supervisor has no terminal: what it would print about its own failure
/// goes beside the job's record, as `<record>.err`.
fn supervise(world: &std::path::Path, record: &std::path::Path) -> anyhow::Result<()> {
    let diagnostics = record.with_extension("err");
    let for_panics = diagnostics.clone();
    std::panic::set_hook(Box::new(move |info| append(&for_panics, &format!("panicked: {info}"))));
    let fastctx_dir = std::env::current_exe()?
        .parent()
        .map(PathBuf::from)
        .ok_or_else(|| anyhow::anyhow!("fastctx's own path has no directory"))?;
    fastctx_hands::run::supervise(world, record, &fastctx_dir)
        .inspect_err(|error| append(&diagnostics, &error.to_string()))?;
    Ok(())
}

fn append(path: &std::path::Path, text: &str) {
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{text}");
    }
}

fn parent_process() -> anyhow::Result<()> {
    let identity = fastctx_platform::host_process::parent_identity()?;
    println!("{}", serde_json::to_string(&identity)?);
    Ok(())
}
