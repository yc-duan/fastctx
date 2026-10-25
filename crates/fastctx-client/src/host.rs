//! Running an agent's own host program in the user's terminal, as
//! `fastctx attach` and the TUI do: it has the terminal to itself until it
//! exits.

use std::io;
use std::process::{Command, ExitStatus};

use fastctx_types::calls::HostCommand;

/// Runs `command` with this terminal as its own, and waits for it.
pub fn run(command: &HostCommand) -> io::Result<ExitStatus> {
    // The TUI calls from its runtime; the CLI does not. A dedicated thread
    // owns the lease's reactor in either case, without nesting runtimes.
    std::thread::scope(|scope| {
        scope
            .spawn(|| run_attached(command))
            .join()
            .map_err(|_| io::Error::other("the host launcher thread panicked"))?
    })
}

fn run_attached(command: &HostCommand) -> io::Result<ExitStatus> {
    // Keeping the IPC socket and its runtime alive holds the host only for
    // this invocation. An older node advertises no lease; attach still works.
    let _lease = command.hold.as_ref().and_then(hold);
    Command::new(&command.program)
        .args(&command.args)
        .envs(command.env.iter().map(|(name, value)| (name, value)))
        .status()
        .map_err(|error| io::Error::new(error.kind(), format!("starting {}: {error}", command.program.display())))
}

fn hold(id: &fastctx_types::ResidentId) -> Option<(tokio::runtime::Runtime, crate::Client)> {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().ok()?;
    let world = crate::World::here().ok()?;
    let client = runtime
        .block_on(async {
            tokio::time::timeout(std::time::Duration::from_secs(15), async {
                let mut client = crate::Client::connect(
                    world.dir(),
                    fastctx_types::ipc::Peer::Command { resident: None, session: None },
                )
                .await
                .ok()?;
                match client.call(fastctx_types::Call::HoldHost { resident: id.clone() }).await.ok()? {
                    fastctx_types::Reply::Done => Some(client),
                    _ => None,
                }
            })
            .await
        })
        .ok()??;
    Some((runtime, client))
}
