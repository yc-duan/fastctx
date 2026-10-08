//! The `fastctx` command.

use std::io::Write as _;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

mod apply;
mod ask;
mod attach;
mod copy;
mod journal;
mod keep;
mod on;
mod page;
mod paths;
mod project;
mod renew;
mod scope_args;
mod server_install;
mod service;
mod stop;
mod talk;
mod update;
mod view;
mod w;
mod world;
mod writers;

#[derive(Parser)]
#[command(name = "fastctx", bin_name = "fastctx", version, about = "One world, many machines.")]
struct Cli {
    /// Without a command, in a terminal, the TUI opens.
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Open the World in this terminal: projects, agents, machines, and a
    /// conversation with any agent on any machine.
    Ui,
    /// Identifies the process invoking the World mod helper.
    #[command(hide = true)]
    ParentProcess,
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
    /// Identifies the node lifetime that may issue queued Claude controls.
    #[command(hide = true)]
    ClaudeControlOwner {
        #[arg(value_parser = paths::parser())]
        world: PathBuf,
    },
    /// A lifetime witness for a Claude mod activation.
    #[command(hide = true)]
    ClaudeControlGuard,
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
        #[arg(long, hide = true, value_parser = ["s4u", "interactive"])]
        service_identity: Option<String>,
        #[arg(long, hide = true)]
        successor: Option<String>,
    },
    /// Replace this machine's node and guardian using a local executable.
    Renew(renew::RenewalArgs),
    /// Install, uninstall or inspect a node/hub service on this machine.
    Service(service::ServiceArgs),
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
    /// turn; with ids, one session or job.
    W(w::WArgs),
    /// Run part of a script on another of the user's machines.
    #[command(override_usage = on::USAGE, after_help = on::AFTER_HELP)]
    On(on::OnArgs),
    /// Stop a job and everything it started, on whichever machine it runs.
    Stop(stop::StopArgs),
    /// List the user's machines: which are online and what each is like.
    Nodes,
    /// Copy a file or a directory from one of the user's machines to another.
    ///
    /// Each end is MACHINE:PATH, or a path on this machine. The destination
    /// path is what the copy becomes; ending it with / copies into that
    /// directory instead. Files already there with the same contents are not
    /// sent again, so running the same copy again sends only what changed.
    /// Links are copied as links. The bytes go between the machines, never
    /// through the terminal.
    Copy(copy::CopyArgs),
    /// Talk to a resident on any machine, or open a session to talk to.
    Talk(talk::TalkArgs),
    /// Open a resident of this machine in its agent's own interface, in
    /// this terminal.
    Attach(attach::AttachArgs),
    /// The user's projects: list, show, map and enter them; open one or
    /// write the next version of its record.
    ///
    /// A fastctx project is not a folder or a repository: it is a named
    /// piece of the user's work that can span several machines. Its record
    /// says what it is for, which machines it spans, and where its code and
    /// data are on each; its journals hold the history of the work, and its
    /// pages show live facts to the user. Its map puts all of that in one
    /// answer.
    #[command(subcommand)]
    Project(project::ProjectCommand),
    /// A project's journals: history, not verdicts. Add an entry, describe
    /// a journal, read or list them.
    #[command(subcommand, after_long_help = fastctx_hands::town::JOURNALS)]
    Journal(journal::JournalCommand),
    /// A project's pages: HTML the user opens in the World app, showing
    /// live facts of the machines and sessions. Publish, read or list them.
    #[command(subcommand, after_long_help = page::PAGE_HELP)]
    Page(page::PageCommand),
    /// Join a World with a code.
    Join(world::JoinArgs),
    /// Make a code that lets one more machine join this World.
    Invite(world::InviteArgs),
    /// Set what a member of the World may do, as a machine that holds
    /// management rights.
    Grant(world::GrantArgs),
    /// Set where a member of the World connects: through relays, or straight
    /// to the hub, as a machine that holds management rights.
    Route(world::RouteArgs),
    /// Make a machine of the World relay for the machines whose routes lead
    /// through it, or stop relaying.
    Relay(world::RelayArgs),
    /// Stop every machine of the World trusting a machine's key, as when it
    /// is lost.
    Revoke(world::RevokeArgs),
    /// Leave the World; this machine becomes a World of one again.
    Leave,
    /// The hub every machine of a World connects to.
    #[command(subcommand)]
    Hub(world::HubCommand),
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
    // An old fastctx's upgrader may have started this version: it waits to
    // hear that it is healthy before anything else happens.
    if let Err(error) = update::finalize() {
        eprintln!("fastctx: {error:#}");
        return ExitCode::FAILURE;
    }
    let cli = match <Cli as clap::FromArgMatches>::from_arg_matches(&command_line().get_matches()) {
        Ok(cli) => cli,
        Err(error) => error.exit(),
    };
    let Some(command) = cli.command else {
        // The TUI needs a terminal on both sides; anything else asked for
        // help, as a command without arguments does.
        use std::io::IsTerminal as _;
        if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
            return finish(fastctx_tui::run().map(|()| ExitCode::SUCCESS));
        }
        let _ = command_line().print_help();
        return ExitCode::from(2);
    };
    let result = match command {
        Command::Ui => fastctx_tui::run(),
        Command::ParentProcess => parent_process(),
        Command::ClaudeRequest { directory } => {
            fastctx_node::claude_request::run(&directory, false).map_err(Into::into)
        }
        Command::ClaudeElicitation { world } => fastctx_node::claude_request::run(&world, true).map_err(Into::into),
        Command::ClaudeControlOwner { world } => fastctx_node::claude_request::owner(&world).map_err(Into::into),
        Command::ClaudeControlGuard => fastctx_node::claude_request::guard().map_err(Into::into),
        Command::Node { dir, supervised, supervisor_control, supervisor_events, service_identity, successor } => {
            let supervision = match (supervised, supervisor_control, supervisor_events) {
                (true, Some(control), Some(events)) => Some((control, events, service_identity)),
                _ => None,
            };
            node(dir, supervision, successor)
        }
        Command::Renew(args) => renew::run(args),
        Command::Service(args) => service::run(args),
        Command::Server(command) => server_install::run(command),
        #[cfg(windows)]
        Command::ServiceHost { name } => fastctx_install::service::run_host(&name),
        #[cfg(windows)]
        Command::ServiceCheck { name } => fastctx_install::service::check_guardian(&name),
        Command::Serve { writer: _, writes } => serve(writes == "off"),
        Command::View(args) => view::run(args),
        Command::Edit(args) => return finish(writers::edit(args)),
        Command::Write(args) => return finish(writers::write(args)),
        Command::ApplyPatch => return finish(writers::apply_patch()),
        Command::W(args) => w::run(args),
        Command::On(args) => return on::run(args),
        Command::Stop(args) => return finish(stop::run(args)),
        Command::Nodes => world::nodes(),
        Command::Copy(args) => return finish(copy::run(args)),
        Command::Talk(args) => talk::run(args),
        Command::Attach(args) => attach::run(args),
        Command::Project(command) => project::run(command),
        Command::Journal(command) => journal::run(command),
        Command::Page(command) => page::run(command),
        Command::Join(args) => world::join(args),
        Command::Invite(args) => world::invite(args),
        Command::Grant(args) => world::grant(args),
        Command::Route(args) => world::route(args),
        Command::Relay(args) => world::relay(args),
        Command::Revoke(args) => world::revoke(args),
        Command::Leave => world::leave(),
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

/// The commands a person runs on a machine: its node, joining and managing
/// the World, talking to residents, the TUI.
const PEOPLE_ONLY: [&str; 18] = [
    "ui", "node", "renew", "service", "server", "serve", "talk", "attach", "join", "invite", "grant", "route", "relay",
    "revoke", "leave", "hub", "apply", "unapply",
];

/// The command line; in a resident's shell its help lists what a resident
/// works with, the others staying callable.
fn command_line() -> clap::Command {
    let mut command = <Cli as clap::CommandFactory>::command();
    if std::env::var_os("FASTCTX_RESIDENT").is_some() {
        command = command.about("Commands for working on this machine and the user's other machines.");
        for name in PEOPLE_ONLY {
            command = command.mut_subcommand(name, |subcommand| subcommand.hide(true));
        }
    }
    command
}

/// A command that sets its own exit code, or fails.
fn finish(result: anyhow::Result<ExitCode>) -> ExitCode {
    result.unwrap_or_else(|error| {
        eprintln!("fastctx: {error:#}");
        ExitCode::FAILURE
    })
}

fn node(
    dir: Option<PathBuf>,
    supervision: Option<(u64, u64, Option<String>)>,
    successor: Option<String>,
) -> anyhow::Result<()> {
    let root = match dir {
        Some(root) => root,
        None => fastctx_platform::fastctx_root()?,
    };
    // Recovery must also start when a newly installed node fails before it
    // can serve. A prepared successor is already watched by that worker.
    if successor.is_none() {
        match fastctx_install::renewal::read(&root) {
            Ok(Some(plan))
                if !matches!(
                    plan.phase,
                    fastctx_types::renewal::Phase::Complete | fastctx_types::renewal::Phase::Failed
                ) =>
            {
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
                _ => fastctx_node::handoff::Identity::Interactive,
            };
            Some(fastctx_node::handoff::Supervision { control, events, identity })
        }
        None => None,
    };
    tokio::runtime::Builder::new_multi_thread().enable_all().build()?.block_on(fastctx_node::run_replacing(
        &root,
        supervision,
        successor.as_deref(),
    ))
}

fn serve(writes_off: bool) -> anyhow::Result<()> {
    let world = fastctx_types::layout::WorldDir::under(&fastctx_platform::fastctx_root()?);
    tokio::runtime::Builder::new_multi_thread().enable_all().build()?.block_on(fastctx_serve::serve(world, writes_off))
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
