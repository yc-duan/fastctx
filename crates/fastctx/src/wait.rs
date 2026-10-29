//! `fastctx wait`: wait until jobs end, as bash's `wait` does for a shell's
//! own children (`world-run.md`).

use std::io::Write as _;
use std::path::Path;
use std::process::ExitCode;

use anyhow::Context as _;
use clap::Args;
use fastctx_client::{CallError, Client, command_peer};
use fastctx_types::layout::WorldDir;
use fastctx_types::residents::Notice;
use fastctx_types::run::{EndedHow, ExitStatus, JobRecord};
use fastctx_types::run_facts::{StepDid, StepEnd, WaitedJob, clipped, tail_of};
use fastctx_types::{Call, JobId, Reply};

#[derive(Args)]
pub struct WaitArgs {
    /// Jobs of this machine, as `run` and `fastctx w` name them, such as
    /// j-k3x9a2.
    #[arg(value_name = "JOB", required = true)]
    jobs: Vec<String>,
}

pub fn run(args: WaitArgs) -> anyhow::Result<ExitCode> {
    let mut jobs = Vec::new();
    for named in &args.jobs {
        if let Some((job, machine)) = named.split_once('@') {
            anyhow::bail!("{named}: wait for a job where it runs: on {machine} fastctx wait {job}");
        }
        jobs.push(JobId::try_from(named.as_str()).with_context(|| named.clone())?);
    }
    let world = WorldDir::under(&fastctx_platform::fastctx_root()?);
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    let mut client =
        runtime.block_on(Client::connect(&world, command_peer()?)).context("reaching this machine's node")?;
    let parent = std::env::var("FASTCTX_JOB").ok().map(|job| job.parse()).transpose()?;
    let began = crate::steps::began();
    let records = match runtime.block_on(client.call(Call::Wait { jobs, receipt: true, parent })) {
        Ok(Reply::Waited { records }) => records,
        Ok(other) => anyhow::bail!("the node answered something else: {other:?}"),
        Err(CallError::Failed(failure)) => anyhow::bail!("{}", failure.message),
        Err(error) => {
            return Err(anyhow::Error::from(error)
                .context("the node stopped answering before the jobs ended; their ends reach you as usual"));
        }
    };
    let mut out = std::io::stdout().lock();
    for record in &records {
        writeln!(out, "{}", fastctx_hands::told::told(&ended(record), None))?;
    }
    out.flush()?;
    drop(out);
    runtime.block_on(client.take_wait()).context("acknowledging the jobs' ends")?;
    if let Some(began) = began {
        began.tell(StepDid::Wait { jobs: records.iter().filter_map(waited).collect() });
    }
    Ok(records.last().map_or(ExitCode::SUCCESS, status))
}

/// A job's end as the wait's step tells it.
fn waited(record: &JobRecord) -> Option<WaitedJob> {
    let tail =
        fastctx_hands::run::last::last_streams(Path::new(&record.output), record.stderr.as_deref().map(Path::new))
            .ok()
            .flatten()
            .map(|last| tail_of(&last.text, 20));
    Some(WaitedJob {
        job: record.id.clone(),
        machine: record.machine.clone(),
        at: record.end.as_ref()?.at,
        end: StepEnd::of_record(record)?,
        tail: tail.filter(|tail| !tail.trim().is_empty()),
        command: Some(clipped(record.script.trim())).filter(|command| !command.is_empty()),
    })
}

/// A job's end as a notice tells it, so a wait reads like the word it took.
fn ended(record: &JobRecord) -> Notice {
    let end = record.end.as_ref().expect("a wait answers with ended jobs");
    Notice::JobEnded {
        taken: false,
        consumed: Vec::new(),
        follower_unconfirmed: false,
        job: record.id.clone(),
        machine: record.machine.clone(),
        request: record.on.as_ref().and_then(|on| on.request.clone()),
        follower: record.on.as_ref().and_then(|on| on.follower.clone()),
        how: end.how.clone(),
        status: record.exit.map(|exit| exit.status),
        at: end.at,
        output: record.output.clone(),
        stderr: record.stderr.clone(),
        last: fastctx_hands::run::last::last_streams(
            Path::new(&record.output),
            record.stderr.as_deref().map(Path::new),
        )
        .unwrap_or(None),
        stop: record.stop.as_ref().map(|stop| Box::new(stop.request.clone())),
        scratch: Vec::new(),
    }
}

/// The status bash's `wait` gives: the exit code, 128 and the signal for a
/// signal, and failure for a job that did not finish on its own.
fn status(record: &JobRecord) -> ExitCode {
    match (record.end.as_ref().map(|end| &end.how), record.exit.map(|exit| exit.status)) {
        (Some(EndedHow::Finished), Some(ExitStatus::Code(code))) => ExitCode::from(code as u8),
        (Some(EndedHow::Finished), Some(ExitStatus::Signal(signal))) => {
            ExitCode::from(128u8.wrapping_add(signal as u8))
        }
        _ => ExitCode::FAILURE,
    }
}
