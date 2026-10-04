//! The `fastctx` command.

use std::io::Write as _;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

mod view;
mod w;
mod world;

#[derive(Parser)]
#[command(name = "fastctx", version, about = "One world, many machines.")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run this machine's node in the foreground.
    Node,
    /// Serve the World's tools to an agent session over MCP's stdio.
    Serve,
    /// Look at files: text with line numbers, images as images.
    View(view::ViewArgs),
    /// See who is here and what each is doing.
    W(w::WArgs),
    /// See the World's machines: which are online and what each is like.
    Nodes,
    /// Join a World with a code.
    Join(world::JoinArgs),
    /// Make a code that lets one more machine join this World.
    Invite(world::InviteArgs),
    /// Stop every machine of the World trusting a machine's key, as when it
    /// is lost.
    Revoke(world::RevokeArgs),
    /// Leave the World; this machine becomes a World of one again.
    Leave,
    /// The hub every machine of a World connects to.
    #[command(subcommand)]
    Hub(world::HubCommand),
    /// Supervise one job; started by the node, not by people.
    #[command(hide = true)]
    Supervise { world: PathBuf, record: PathBuf },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Node => node(),
        Command::Serve => serve(),
        Command::View(args) => view::run(args),
        Command::W(args) => w::run(args),
        Command::Nodes => world::nodes(),
        Command::Join(args) => world::join(args),
        Command::Invite(args) => world::invite(args),
        Command::Revoke(args) => world::revoke(args),
        Command::Leave => world::leave(),
        Command::Hub(command) => world::hub(command),
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

fn node() -> anyhow::Result<()> {
    let root = fastctx_platform::fastctx_root()?;
    tokio::runtime::Builder::new_multi_thread().enable_all().build()?.block_on(fastctx_node::run(&root))
}

fn serve() -> anyhow::Result<()> {
    let world = fastctx_types::layout::WorldDir::under(&fastctx_platform::fastctx_root()?);
    tokio::runtime::Builder::new_multi_thread().enable_all().build()?.block_on(fastctx_serve::serve(world))
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
