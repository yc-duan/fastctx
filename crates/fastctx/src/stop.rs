//! `fastctx stop`: a job and everything it started, on whichever machine it
//! runs (`world-run.md`).

use std::process::ExitCode;

use anyhow::Context as _;
use clap::Args;
use fastctx_client::{CallError, Client, command_peer};
use fastctx_types::layout::WorldDir;
use fastctx_types::run::EndedHow;
use fastctx_types::{Call, JobId, MachineName, Reply};

/// Stop a job and everything it started, on whichever machine it runs.
#[derive(Args)]
pub struct StopArgs {
    /// The job, as `fastctx w` shows it, such as j-k3x9a2; j-k3x9a2@laptop
    /// names the machine it runs on, when jobs on several have this id.
    job: String,
    /// The machine, written the older way.
    #[arg(long, hide = true)]
    machine: Option<String>,
}

pub fn run(args: StopArgs) -> anyhow::Result<ExitCode> {
    let (job, named) = match args.job.split_once('@') {
        Some((job, machine)) => (job, Some(machine)),
        None => (args.job.as_str(), None),
    };
    let job = JobId::try_from(job)?;
    let machine = named.or(args.machine.as_deref()).map(MachineName::try_from).transpose()?;
    let world = WorldDir::under(&fastctx_platform::fastctx_root()?);
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    let reply = runtime.block_on(async {
        let mut client = Client::connect(&world, command_peer()).await.context("reaching this machine's node")?;
        anyhow::Ok(client.call(Call::StopJob { job: job.clone(), machine }).await)
    })?;
    match reply {
        Ok(Reply::JobStopped { machine, record }) => {
            let how = match record.end.as_ref().map(|end| &end.how) {
                Some(EndedHow::Stopped) if record.stop.is_some() => "stopped".to_string(),
                Some(EndedHow::Stopped) => "already stopped".to_string(),
                Some(EndedHow::Finished) => match record.exit {
                    Some(exit) => format!("had already ended: {}", exit.status),
                    None => "had already ended".to_string(),
                },
                Some(EndedHow::Interrupted { reason }) => format!("had already been interrupted: {reason}"),
                Some(EndedHow::Failed { error }) => format!("could not be stopped cleanly: {error}"),
                None => "still running".to_string(),
            };
            println!("=== stop {job} on {machine} ({how}) ===");
            Ok(if matches!(record.end.as_ref().map(|end| &end.how), Some(EndedHow::Failed { .. }) | None) {
                ExitCode::FAILURE
            } else {
                ExitCode::SUCCESS
            })
        }
        Ok(other) => anyhow::bail!("the node answered something else: {other:?}"),
        Err(CallError::Failed(failure)) => {
            eprintln!("fastctx stop: {}", failure.message);
            Ok(ExitCode::FAILURE)
        }
        Err(error) => Err(error.into()),
    }
}
