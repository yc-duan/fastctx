//! Call facts survive their large artifacts and are attached by explicit host identity only.

mod cursor;
#[cfg(feature = "storage-drill")]
pub(crate) mod drill;
#[cfg(feature = "storage-drill")]
mod drill_mixed;
mod journal;
mod log;
mod migrate;
#[cfg(feature = "storage-drill")]
pub(crate) mod migration_drill;
mod projections;
mod retirement;
mod snapshot;
mod sql;
mod steps;
pub(crate) mod storage;
pub(crate) use retirement::SCHEMA as RETIREMENT_SCHEMA;
pub(crate) use sql::SCHEMA;
pub(crate) mod output;
pub(crate) mod updates;
pub(crate) mod visits;
pub(crate) mod writing;
pub(crate) use steps::tell as tell_step;
pub(crate) use storage::{evidence, job_updates, observations, pending_machines, stamp};
mod acceptance;
mod asynchronous;
pub(crate) use asynchronous::{begin_for, failed_for, keep_for, keep_reply, legacy_reply, receive, record_failure};
pub(crate) use updates::job as update_job;

use crate::Node;
use fastctx_platform::{Permissions, publish, publish_new};
use fastctx_types::layout::ConversationDir;
use fastctx_types::run::{OutputPiece, RunReply};
use fastctx_types::run_facts::{OutputRef, RunFacts, WriteFact};
use fastctx_types::session::{Happened, SessionEvent, What};
use std::io;
use std::path::Path;

static WRITING: std::sync::LazyLock<crate::record_gates::Gates> = std::sync::LazyLock::new(|| {
    crate::record_gates::Gates::new(fastctx_platform::doing::Rank::WRITE_FACTS, "write facts")
});

pub(crate) use writing::keep as write;

pub(crate) fn record_visit(path: &Path, bytes: &[u8]) -> io::Result<()> {
    visits::keep(path, bytes)
}

pub(crate) fn writes(record: &Path) -> io::Result<Vec<WriteFact>> {
    writing::prepare(record)?.all("writes")
}

/// The machines the job of `record` went to with `on`, and the jobs it
/// started there.
pub(crate) fn visits(record: &Path) -> io::Result<Vec<(fastctx_types::MachineName, fastctx_types::JobId)>> {
    visits::all(record)
}

/// The most bytes of changes' rows one run keeps with the conversation.
const RUN_INLINE: u64 = 256 * 1024;

pub(crate) fn assemble(reply: &RunReply, record: &Path) -> io::Result<RunFacts> {
    let cut = reply
        .output
        .pieces
        .iter()
        .any(|piece| matches!(piece, OutputPiece::Omitted { .. } | OutputPiece::PartOfLine { .. }));
    let mut writes = writes(record)?;
    // What one run keeps with the conversation of its changes' rows is
    // bounded, all of them together: past it, a change is read from its
    // machine.
    let mut kept = 0;
    for diff in writes.iter_mut().filter_map(|write| write.diff.as_mut()).filter(|diff| diff.lines.is_some()) {
        kept += diff.bytes.unwrap_or(fastctx_types::run_facts::INLINE_DIFF);
        if kept > RUN_INLINE {
            diff.lines = None;
        }
    }
    let (spans, notes) = fastctx_hands::run::projection::read_with_matches(&reply.output, &writes)?;
    Ok(RunFacts {
        folded_steps: Vec::new(),
        turn: fastctx_hands::run::record::read(record)?.turn,
        record: Some(record.to_string_lossy().into_owned()),
        machine: reply.machine.clone(),
        state: reply.state.clone(),
        job: Some(reply.job.clone()),
        output: Some(OutputRef {
            policy: Some(fastctx_hands::lines::output_policy()),
            content: if fastctx_hands::run::record::read(record)?.output_ended_at.is_some() {
                Some(fastctx_hands::files::content_fingerprint(Path::new(&reply.output.log))?)
            } else {
                None
            },
            machine: reply.machine.clone(),
            path: reply.output.log.clone(),
            bytes: reply.output.bytes,
            lines: reply.output.lines,
            cut,
        }),
        visited: reply.visited.clone(),
        writes,
        spans,
        notes,
        steps: steps::steps(record)?,
    })
}

pub(crate) fn attach(directory: &ConversationDir, event: &mut SessionEvent) -> io::Result<()> {
    let item = match &mut event.happened {
        Happened::Began { item, .. } | Happened::Updated { item, .. } | Happened::Finished { item, .. } => item,
        _ => return Ok(()),
    };
    let What::Tool(tool) = &mut item.what else { return Ok(()) };
    if tool.name != "run"
        || !tool.server.as_deref().is_some_and(|server| server == "fastctx" || server.ends_with("_fastctx"))
    {
        return Ok(());
    }
    let Some(facts) = storage::get(directory, &item.id)? else { return Ok(()) };
    tool.machine = Some(facts.machine.clone());
    if let fastctx_types::session::Does::Command { exit, job, .. } = &mut tool.does {
        *exit = match &facts.state {
            fastctx_types::run::RunState::Ended { status: fastctx_types::run::ExitStatus::Code(code) }
            | fastctx_types::run::RunState::Exited { status: fastctx_types::run::ExitStatus::Code(code), .. } => {
                Some(*code)
            }
            _ => None,
        };
        *job = if matches!(
            facts.state,
            fastctx_types::run::RunState::Running | fastctx_types::run::RunState::Exited { remaining: 1.., .. }
        ) {
            facts.job.as_ref().map(ToString::to_string)
        } else {
            None
        };
    }
    tool.run = Some(Box::new(facts));
    Ok(())
}

pub(crate) fn read_since(record: &Path, cursor: &mut u64) -> io::Result<Vec<WriteFact>> {
    writing::prepare(record)?.page("writes", cursor)
}

/// The activity book has its own durable cursor, independent of fact delivery.
pub(crate) fn book_since(
    record: &Path,
    cursor: &mut crate::activity_book::Cursor,
) -> io::Result<(Vec<WriteFact>, Vec<fastctx_types::run_facts::RunStep>)> {
    let writes = read_since(record, &mut cursor.writes_offset)?;
    let gate = WRITING.get(&record.with_extension("facts"));
    let _writing = gate.lock();
    Ok((writes, journal::read_since(record, &mut cursor.steps_segment, &mut cursor.steps_offset)?))
}

pub(crate) fn repair_journal(record: &Path, legacy: bool) -> io::Result<()> {
    writing::recover(record, legacy)
}
