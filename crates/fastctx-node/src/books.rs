//! Retiring an archived conversation removes rebuildable observations and settled
//! retry diagnostics. Configuration, history and delivery proofs retain their
//! owning family's lifetime, even when their old pathname was under books/.

pub(crate) mod cleanup;
pub(crate) use cleanup::resume;

use std::io;

use fastctx_hands::run::record;
use fastctx_types::Address;
use fastctx_types::layout::{ConversationDir, Kind};

use crate::Node;

/// Retires the books of every removed conversation here that owes nothing.
pub fn retire(node: &Node) -> io::Result<()> {
    let began = std::time::Instant::now();
    let mut failures = Vec::new();
    let mut failed = 0;
    let mut waiting = node
        .world
        .kept()?
        .into_iter()
        .filter(|kept| matches!(kept.kind, Kind::Home(_)))
        .map(|kept| (kept, None))
        .collect::<std::collections::VecDeque<_>>();
    while let Some((kept, checked)) = waiting.pop_front() {
        let Kind::Home(id) = &kept.kind else { unreachable!() };
        match retire_one(node, id, &kept.directory, checked) {
            Ok((cleanup::Progress::More, revision)) => waiting.push_back((kept, revision)),
            Ok(_) => {}
            Err(error) => {
                failed += 1;
                if failures.len() < 32 {
                    failures.push(format!("{id}: {error}"));
                }
            }
        }
    }
    fastctx_platform::readings::timing("node.books.retirement_pass").record(began.elapsed());
    if failed > failures.len() {
        failures.push(format!("{} further conversations could not retire", failed - failures.len()));
    }
    if failures.is_empty() { Ok(()) } else { Err(io::Error::other(failures.join("; "))) }
}

fn retire_one(
    node: &Node,
    id: &fastctx_types::ResidentId,
    directory: &ConversationDir,
    checked: Option<u64>,
) -> io::Result<(cleanup::Progress, Option<u64>)> {
    let waiting = (cleanup::Progress::Waiting, None);
    let Some(revision) = node.residents.archived_revision(id) else { return Ok(waiting) };
    if checked != Some(revision) {
        cleanup::prepare_cleanup(directory)?;
        if cleanup::finished(directory)? {
            return Ok(waiting);
        }
        for exchange in crate::retention::entries(&directory.path().join("claude"))? {
            if let Some(registration) = fastctx_hosts::claude::exchange::registration(&exchange)?
                && let Some(process) = registration.process
                && fastctx_platform::is_alive(&process)?
            {
                return Ok(waiting);
            }
        }
        crate::delivery::prepare(directory)?;
        let owner = Address { machine: node.machine(), resident: id.clone() };
        if owes(node, &owner, directory)? {
            return Ok(waiting);
        }
    }
    let Ok(management) = node.residents.management(id) else { return Ok(waiting) };
    let Ok(_management) = management.try_write() else { return Ok(waiting) };
    // A restore followed by another archive invalidates the checks above, even
    // if the wall clock has not advanced. The next pass checks its new jobs.
    if node.residents.archived_revision(id) != Some(revision) {
        return Ok(waiting);
    }
    let Some(speaking) = node.residents.speaking(id) else { return Ok(waiting) };
    let Ok(_speaking) = speaking.try_lock() else { return Ok(waiting) };
    let active = node.residents.host_open(id)
        || node
            .residents
            .now(id)
            .is_some_and(|now| now.activity == fastctx_types::residents::Activity::Busy || now.turn.is_some());
    if active {
        return Ok(waiting);
    }
    let progress = cleanup::retire(directory)?;
    if progress == cleanup::Progress::Retired {
        node.log.line(format_args!("{id} was removed and owes nothing: its rebuildable bookkeeping was retired"));
    }
    Ok((progress, Some(revision)))
}

/// Whether anything of `owner` is still on its way: an answer its readers
/// do not have yet, a job of its that runs here or whose end it is still
/// owed, or a job it left running on another machine.
fn owes(node: &Node, owner: &Address, directory: &ConversationDir) -> io::Result<bool> {
    if crate::records::read(directory, |rows| {
        Ok(rows.connection().query_row(
            "SELECT EXISTS(SELECT 1 FROM terminal_work WHERE conversation=?1)",
            [rows.conversation()],
            |row| row.get::<_, bool>(0),
        )?)
    })? {
        return Ok(true);
    }
    // Goal bindings still identify records that deletion must collect.
    if crate::goals::lifecycle::unfinished(directory)?
        || crate::goals::lifecycle::keeps_books(node, &owner.resident)
            .map_err(|error| io::Error::other(error.message))?
    {
        return Ok(true);
    }
    for queue in
        ["job-deliveries", "on-deliveries", "on-stops", "run-stops", "run-requests", "received", "received-controls"]
    {
        if crate::records::items::page(&directory.book(queue), Some("json"), "", 1)?
            .iter()
            .any(|path| path.extension().is_some_and(|extension| extension == "json"))
        {
            return Ok(true);
        }
    }
    let thread = node.residents.about(&owner.resident).and_then(|(about, _)| about.session);
    if crate::codex::pending(node, directory, thread.as_deref())? {
        return Ok(true);
    }
    if crate::inbox::answers::has_pending(directory)? || crate::stops::has_pending(directory)? {
        return Ok(true);
    }
    {
        for jobs in node.world.job_directories(&owner.machine, owner)? {
            for path in crate::retention::entries(&jobs)? {
                match path.extension().and_then(|extension| extension.to_str()) {
                    Some("json") if crate::job_metadata::exists(&path.with_extension("owed"))? => return Ok(true),
                    Some("json") if record::read(&path)?.end.is_none() => return Ok(true),
                    _ => {}
                }
            }
        }
    }
    crate::here::has_pending(directory)
}
