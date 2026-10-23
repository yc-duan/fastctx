//! Call facts survive their large artifacts and are attached by explicit host identity only.

mod log;
mod steps;
mod storage;
mod updates;
pub(crate) use storage::{
    begin as begin_for, evidence, evidence_all, failed as failed_for, job_updates, legacy_reply, observations, receive, record_failure, stamp,
};
pub(crate) use steps::{steps, tell as tell_step};
pub(crate) use updates::{job as update_job, job_with as update_job_with};

use crate::Node;
use fastctx_platform::{Permissions, publish, publish_new};
use fastctx_types::layout::ConversationDir;
use fastctx_types::run::{OutputPiece, RunReply};
use fastctx_types::run_facts::{OutputRef, RunFacts, WriteFact};
use fastctx_types::session::{Happened, SessionEvent, What};
use std::io;
use std::path::Path;

static WRITING: std::sync::LazyLock<crate::ranked::WriteFacts> = std::sync::LazyLock::new(Default::default);

pub(crate) fn write(directory: &Path, fact: &WriteFact) -> io::Result<()> {
    let _writing = WRITING.lock();
    if fact.id.is_empty() || !fact.id.bytes().all(|byte| byte.is_ascii_hexdigit() || byte == b'-') {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid write fact identity"));
    }
    fastctx_platform::ensure_private_dir(directory)?;
    let path = directory.join(format!("{}.json", fact.id));
    let bytes = serde_json::to_vec(fact)?;
    match publish_new(&path, &bytes, Permissions::Private) {
        Ok(()) => {
            let mut bytes = bytes;
            bytes.push(b'\n');
            log::append(&directory.with_extension("writes.jsonl"), &bytes)
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            if std::fs::read(path)? == bytes {
                Ok(())
            } else {
                Err(io::Error::new(io::ErrorKind::InvalidData, "write fact identity changed"))
            }
        }
        Err(error) => Err(error),
    }
}

pub(crate) fn writes(record: &Path) -> io::Result<Vec<WriteFact>> {
    let mut facts = read_since(record, &mut 0)?;
    let known: std::collections::HashSet<_> = facts.iter().map(|fact| fact.id.clone()).collect();
    let entries = match std::fs::read_dir(record.with_extension("writes")) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(facts),
        Err(error) => return Err(error),
    };
    let mut recovered = Vec::new();
    for entry in entries {
        let path = entry?.path();
        if path.file_stem().and_then(|name| name.to_str()).is_some_and(|id| known.contains(id)) {
            continue;
        }
        recovered.push(serde_json::from_slice(&std::fs::read(path)?)?);
    }
    // A published fact survives an interrupted append; completed appends keep their order.
    recovered.sort_by(|a: &WriteFact, b| a.id.cmp(&b.id));
    facts.extend(recovered);
    Ok(facts)
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

fn path(directory: &ConversationDir, id: &str) -> std::path::PathBuf {
    // Host ids are opaque input, never path components.
    directory.calls().join(format!("{}.json", fastctx_hands::files::content_hash(id.as_bytes())))
}

pub(crate) fn keep(directory: &ConversationDir, id: &str, facts: &RunFacts) -> io::Result<()> {
    storage::keep(directory, id, facts, false)
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

pub(crate) fn keep_for(
    node: &Node,
    resident: &fastctx_types::ResidentId,
    id: &str,
    facts: &RunFacts,
) -> io::Result<()> {
    if !node.residents.knows(resident) {
        return Err(io::Error::other("unknown resident"));
    }
    storage::keep(&node.world.conversation(resident), id, facts, true)
}

pub(crate) fn keep_reply(
    node: &Node,
    resident: &fastctx_types::ResidentId,
    id: &str,
    facts: &RunFacts,
) -> io::Result<()> {
    if !node.residents.knows(resident) {
        return Err(io::Error::other("unknown resident"));
    }
    keep(&node.world.conversation(resident), id, facts)
}

pub(crate) fn read_since(record: &Path, cursor: &mut u64) -> io::Result<Vec<WriteFact>> {
    use std::io::{BufRead, Seek};
    let mut file = match std::fs::File::open(record.with_extension("writes.jsonl")) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    file.seek(std::io::SeekFrom::Start(*cursor))?;
    let mut reader = std::io::BufReader::new(file);
    let mut facts = Vec::new();
    let mut line = String::new();
    loop {
        line.clear();
        let n = reader.read_line(&mut line)?;
        if n == 0 || !line.ends_with('\n') {
            break;
        }
        facts.push(serde_json::from_str(&line).map_err(io::Error::other)?);
        *cursor += n as u64;
    }
    Ok(facts)
}
