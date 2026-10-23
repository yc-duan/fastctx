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

#[derive(Args)]
#[command(about = fastctx_types::collab_prompt::STOP_HELP)]
pub struct StopArgs {
    #[arg(value_name = "ID", required = true, help = fastctx_types::collab_prompt::STOP_IDS)]
    jobs: Vec<String>,
    /// The account whose sentinels are being withdrawn.
    #[arg(long = "as")]
    account: Option<fastctx_types::AccountName>,
    /// The network containing those sentinels.
    #[arg(long)]
    hub: Option<fastctx_types::HubName>,
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
        if named.starts_with("s-") {
            let stopped = match crate::sense::stop(
                &runtime,
                &mut client,
                named,
                args.machine.as_deref(),
                args.hub.as_ref(),
                args.account.as_ref(),
            ) {
                Ok(stopped) => stopped,
                Err(error) => {
                    eprintln!("fastctx stop: {named}: {error:#}");
                    false
                }
            };
            failed |= !stopped;
            continue;
        }
        let stopped = match target(named, args.machine.as_deref()) {
            Ok((Target::Job(job), machine)) => stop(&runtime, &mut client, job, machine)?,
            Ok((Target::Colleague(resident), machine)) => {
                match runtime.block_on(client.call(Call::Stop { resident, machine })) {
                    Ok(Reply::Stopped { resident, stopped }) => {
                        println!("{}", fastctx_types::collab_prompt::stopped(&resident, stopped));
                        true
                    }
                    Ok(other) => anyhow::bail!("the node answered something else: {other:?}"),
                    Err(CallError::Failed(failure)) => {
                        eprintln!("fastctx stop: {named}: {}", failure.message);
                        false
                    }
                    Err(error) => return Err(error.into()),
                }
            }
            Err(error) => {
                eprintln!("fastctx stop: {named}: {error:#}");
                false
            }
        };
        failed |= !stopped;
    }
    Ok(if failed { ExitCode::FAILURE } else { ExitCode::SUCCESS })
}

enum Target {
    Job(JobId),
    Colleague(fastctx_types::ResidentId),
}

fn target(named: &str, machine: Option<&str>) -> anyhow::Result<(Target, Option<Written>)> {
    let (job, named) = match named.split_once('@') {
        Some((job, machine)) => (job, Some(machine)),
        None => (named, None),
    };
    let machine = named.or(machine).map(Written::parse).transpose()?;
    let target =
        if job.starts_with("codex-") { Target::Colleague(job.try_into()?) } else { Target::Job(job.try_into()?) };
    Ok((target, machine))
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
