//! A conversation's bookkeeping (`books/` in its directory: what keeps it from
//! being told the same thing twice, the settings of its host session, what
//! it saw of files) is only of use while the conversation can still be
//! spoken to or its account records still need cleanup. Once the user removed
//! it and neither obligation remains, the books go. Its own records stay.

use std::io;

use fastctx_hands::run::record;
use fastctx_types::Address;
use fastctx_types::layout::{ConversationDir, Kind};

use crate::Node;

/// Retires the books of every removed conversation here that owes nothing.
pub fn retire(node: &Node) -> io::Result<()> {
    let here = node.machine();
    'conversation: for kept in node.world.kept()? {
        let Kind::Home(id) = &kept.kind else { continue };
        let directory = &kept.directory;
        if crate::residents::read_removed(directory)?.is_none()
            || (!directory.books().exists() && directory.path().join("books-retired").exists())
        {
            continue;
        }
        let owner = Address { machine: here.clone(), resident: id.clone() };
        let Some(gate) = node.residents.speaking(id) else { continue };
        let Ok(_speaking) = gate.try_lock() else { continue };
        let active = node.residents.host_open(id)
            || node
                .residents
                .now(id)
                .is_some_and(|now| now.activity == fastctx_types::residents::Activity::Busy || now.turn.is_some());
        if active {
            continue;
        }
        for exchange in crate::retention::entries(&directory.path().join("claude"))? {
            if let Some(registration) = fastctx_hosts::claude::exchange::registration(&exchange)?
                && let Some(process) = registration.process
                && fastctx_platform::is_alive(&process)?
            {
                continue 'conversation;
            }
        }
        let _transaction = crate::resident_storage::lock();
        if owes(node, &owner, directory)? {
            continue;
        }
        fastctx_platform::owned_directory::delete(&directory.outbox())?;
        fastctx_platform::publish(
            &directory.path().join("books-retired"),
            b"",
            fastctx_platform::Permissions::Private,
        )?;
        fastctx_platform::owned_directory::delete(&directory.books())?;
        node.log.line(format_args!("{id} was removed and owes nothing: its bookkeeping went"));
    }
    Ok(())
}

/// Whether anything of `owner` is still on its way: an answer its readers
/// do not have yet, a job of its that runs here or whose end it is still
/// owed, or a job it left running on another machine.
fn owes(node: &Node, owner: &Address, directory: &ConversationDir) -> io::Result<bool> {
    // Goal bindings still identify records that deletion must collect (2026-10-23).
    if crate::goals::lifecycle::unfinished(directory)?
        || crate::goals::lifecycle::keeps_books(node, &owner.resident)
            .map_err(|error| io::Error::other(error.message))?
    {
        return Ok(true);
    }
    for queue in ["job-deliveries", "on-deliveries", "on-stops", "run-stops", "received", "received-controls"] {
        if crate::retention::entries(&directory.book(queue))?
            .iter()
            .any(|path| path.extension().is_some_and(|extension| extension == "json"))
        {
            return Ok(true);
        }
    }
    let reports = directory.books().join("host-reports.json");
    match std::fs::read(&reports) {
        Ok(bytes) => {
            let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
            if value.get("events").and_then(serde_json::Value::as_array).is_some_and(|events| !events.is_empty()) {
                return Ok(true);
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    for path in crate::retention::entries(&directory.outbox())? {
        let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else { continue };
        if path.extension().is_some_and(|extension| extension == "json")
            && !directory.paid().join(stem).try_exists()?
            && !directory.outbox().join(format!("{stem}.paid")).try_exists()?
        {
            return Ok(true);
        }
    }
    for jobs in node.world.job_directories(&owner.machine, owner)? {
        for path in crate::retention::entries(&jobs)? {
            match path.extension().and_then(|extension| extension.to_str()) {
                Some("owed") => return Ok(true),
                Some("json") if record::read(&path)?.end.is_none() => return Ok(true),
                _ => {}
            }
        }
    }
    Ok(crate::retention::entries(&directory.elsewhere())?
        .iter()
        .any(|path| path.extension().is_some_and(|ext| ext == "json")))
}
