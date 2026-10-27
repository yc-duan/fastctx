//! `fastctx stop`: jobs and everything they started, on whichever machine
//! each runs (`world-run.md`).

use std::process::ExitCode;

use anyhow::Context as _;
use clap::Args;
use fastctx_client::{CallError, Client, command_peer};
use fastctx_types::access::Written;
use fastctx_types::layout::WorldDir;
use fastctx_types::run::EndedHow;
use fastctx_types::run_facts::{StepDid, StopHow, clipped};
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
        let began = crate::steps::began();
        if named.starts_with("s-") {
            let stopped = match crate::sense::stop(
                &runtime,
                &mut client,
                named,
                args.machine.as_deref(),
                args.hub.as_ref(),
                args.account.as_ref(),
            ) {
                Ok(stopped) => {
                    tell(began, named, None, if stopped { StopHow::Stopped } else { StopHow::NotCleanly });
                    stopped
                }
                Err(error) => {
                    eprintln!("fastctx stop: {named}: {error:#}");
                    false
                }
            };
            failed |= !stopped;
            continue;
        }
        let stopped = match target(named, args.machine.as_deref()) {
            Ok((Target::Job(job), machine)) => match stop(&runtime, &mut client, job, machine)? {
                Some((machine, how)) => {
                    tell(began, named, Some(machine), how);
                    !matches!(how, StopHow::NotCleanly | StopHow::StillRunning)
                }
                None => false,
            },
            Ok((Target::Colleague(resident), machine)) => {
                match runtime.block_on(client.call(Call::Stop { resident, machine })) {
                    Ok(Reply::Stopped { resident, stopped }) => {
                        println!("{}", fastctx_types::collab_prompt::stopped(&resident, stopped));
                        tell(began, named, None, if stopped { StopHow::Stopped } else { StopHow::AlreadyStopped });
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

/// The step of a stop, where a run gave it.
fn tell(began: Option<crate::steps::Began>, what: &str, on: Option<fastctx_types::MachineName>, how: StopHow) {
    if let Some(began) = began {
        began.tell(StepDid::Stop { what: clipped(what), on, how });
    }
}

/// Where the job was and how the stop left it, said in one line; none when
/// it could not be asked.
fn stop(
    runtime: &tokio::runtime::Runtime,
    client: &mut Client,
    job: JobId,
    machine: Option<Written>,
) -> anyhow::Result<Option<(fastctx_types::MachineName, StopHow)>> {
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
            Ok(Some((machine, StopHow::of_record(&record))))
        }
        Ok(other) => anyhow::bail!("the node answered something else: {other:?}"),
        Err(CallError::Failed(failure)) => {
            eprintln!("fastctx stop: {job}: {}", failure.message);
            Ok(None)
        }
        Err(error) => Err(error.into()),
    }
}
