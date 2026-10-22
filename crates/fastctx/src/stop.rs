//! `fastctx stop`: jobs and everything they started, on whichever machine
//! each runs (`world-run.md`).

use std::process::ExitCode;

use anyhow::Context as _;
use clap::Args;
use fastctx_client::{CallError, Client, command_peer};
use fastctx_types::access::Written;
use fastctx_types::layout::WorldDir;
use fastctx_types::run::EndedHow;
use fastctx_types::{Call, JobId, Reply};

/// Stop jobs and everything they started, on whichever machine each runs.
#[derive(Args)]
pub struct StopArgs {
    /// The jobs, as `fastctx w` shows them, such as j-k3x9a2; j-k3x9a2@laptop
    /// names the machine it runs on, when jobs on several have this id.
    #[arg(value_name = "JOB", required = true)]
    jobs: Vec<String>,
    /// The machine, written the older way.
    #[arg(long, hide = true)]
    machine: Option<String>,
}

/// Each job is stopped on its own, as `kill` does: one that cannot be is said
/// and the rest are still stopped, and the command fails if any could not.
pub fn run(args: StopArgs) -> anyhow::Result<ExitCode> {
    let world = WorldDir::under(&fastctx_platform::fastctx_root()?);
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    let mut client =
        runtime.block_on(Client::connect(&world, command_peer()?)).context("reaching this machine's node")?;
    let mut failed = false;
    for named in &args.jobs {
        let stopped = match target(named, args.machine.as_deref()) {
            Ok((job, machine)) => stop(&runtime, &mut client, job, machine)?,
            Err(error) => {
                eprintln!("fastctx stop: {named}: {error:#}");
                false
            }
        };
        failed |= !stopped;
    }
    Ok(if failed { ExitCode::FAILURE } else { ExitCode::SUCCESS })
}

fn target(named: &str, machine: Option<&str>) -> anyhow::Result<(JobId, Option<Written>)> {
    let (job, named) = match named.split_once('@') {
        Some((job, machine)) => (job, Some(machine)),
        None => (named, None),
    };
    let machine = named.or(machine).map(Written::parse).transpose()?;
    Ok((JobId::try_from(job)?, machine))
}

/// Whether the job is no longer running, said in one line either way.
fn stop(
    runtime: &tokio::runtime::Runtime,
    client: &mut Client,
    job: JobId,
    machine: Option<Written>,
) -> anyhow::Result<bool> {
    let (machine, hub) = match machine {
        Some(written) => (Some(written.name), written.hub),
        None => (None, None),
    };
    let reply = runtime.block_on(client.call(Call::StopJob { job: job.clone(), machine, hub }));
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
            Ok(!matches!(record.end.as_ref().map(|end| &end.how), Some(EndedHow::Failed { .. }) | None))
        }
        Ok(other) => anyhow::bail!("the node answered something else: {other:?}"),
        Err(CallError::Failed(failure)) => {
            eprintln!("fastctx stop: {job}: {}", failure.message);
            Ok(false)
        }
        Err(error) => Err(error.into()),
    }
}
