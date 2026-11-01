//! Discovery names only node-owned records. It never interprets arbitrary files
//! in a conversation, page, attachment directory or job output as SQL metadata.
use crate::Node;
use fastctx_platform::disk;
use fastctx_types::layout::Kind;
use std::{io, path::Path, sync::Arc};

const QUEUES: &[&str] =
    &["received", "received-controls", "job-deliveries", "on-deliveries", "on-stops", "run-stops", "run-requests"];
const SCALARS: &[&str] = &[
    "owed",
    "notice",
    "delivered",
    "wait-taken",
    "on-taken",
    "on-stopped",
    "on-piped",
    "answer-wait",
    "posted",
    "copied",
    "run-request",
    "facts-state",
    "facts-pending",
    "facts-output",
    "done",
    "retry",
];

pub(super) fn scan(node: &Arc<Node>) -> io::Result<bool> {
    let mut ready = true;
    let mut prepare = |result: io::Result<()>| match result {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => ready = false,
        Err(error) => {
            ready = false;
            node.log.line(format_args!("migrating legacy records: {error}"));
        }
    };
    for kept in node.world.kept()? {
        let directory = &kept.directory;
        if node
            .database
            .read_background_now(|db| crate::conversation_store::deleted(db, &kept.name))
            .map_err(io::Error::other)?
        {
            continue;
        }
        prepare(crate::session_record::prepare(directory));
        prepare(crate::run_facts::storage::database(directory).map(|_| ()));
        prepare(crate::delivery::prepare(directory));
        prepare(crate::inbox::storage::initialize(directory));
        prepare(crate::inbox::outgoing::prepare(directory));
        prepare(crate::control_queue::storage::initialize(directory));
        prepare(crate::stops::store::prepare(directory));
        for family in ["actions", "messages", "seen"] {
            prepare(crate::residents::activity::prepare(directory, family));
        }
        prepare(crate::attachments::delivered::prepare(directory));
        prepare(crate::claude::turn_facts::storage::migrate(directory));
        prepare(crate::codex::native_context::prepare(directory));
        prepare(crate::goals::intents::prepare(directory));
        prepare(crate::goals::local::prepare(directory));
        prepare(crate::goals::report_store::prepare(directory));
        prepare(crate::goals::replies::initialize(directory));
        prepare(crate::goals::scope::initialize(directory));
        prepare(crate::lifecycle::refused::prepare(directory));
        for folder in ["user-stops", "turn-stops", "turn-failures", "goal-compactions", "goal-inputs"] {
            for path in entries(&directory.book(folder))? {
                if folder == "goal-compactions"
                    && path.file_name().is_some_and(|name| {
                        let name = name.to_string_lossy();
                        name.ends_with(".control.json") || name.ends_with(".reply.json")
                    })
                {
                    continue;
                }
                if disk::symlink_metadata(&path)?.is_file() && !hidden(&path) {
                    prepare(crate::records::metadata::prepare(&path).map(|_| ()));
                }
            }
        }
        let retry = directory.books().join("goal-retry.json");
        if disk::exists(&retry)? {
            prepare(crate::records::metadata::prepare(&retry).map(|_| ()));
        }
        for queue in QUEUES {
            prepare(crate::records::items::prepare(&directory.book(queue).join(".items")).map(|_| ()));
        }
        if let Kind::Home(id) = &kept.kind {
            prepare(crate::context::records::prepare(&node.world, id).map(|_| ()));
            prepare(crate::codex::native_lifetime::prepare(&node.world, id).map(|_| ()));
            prepare(crate::scratch::storage::prepare(&node.world, id).map(|_| ()));
            prepare(crate::run_receipts::prepare(node, id).map(|_| ()));
            prepare(crate::lifecycle::retry::directory(node, id).map(|_| ()));
            if let Some((about, _)) = node.residents.about(id)
                && about.host.agent.as_str() == "codex"
                && let Some(thread) = about.session
            {
                prepare(crate::codex::report_store::prepare(node, &thread).map(|_| ()));
            }
        }
    }
    for queue in QUEUES {
        prepare(crate::records::items::prepare(&node.world.path().join(queue).join(".items")).map(|_| ()));
    }
    prepare(crate::control_posts::requests::prepare(node).map(|_| ()));
    prepare(crate::timers::store::Store::open(&node.world.path().join("timers")).map(|_| ()));
    for parent in node.world.job_parents()? {
        for owner in entries(&parent)? {
            for path in entries(&owner.join("jobs"))? {
                if path.extension().is_none_or(|ext| ext != "json") || hidden(&path) {
                    continue;
                }
                let job = match crate::job_metadata::Job::open(&path) {
                    Ok(job) => job,
                    Err(error) => {
                        node.log.line(format_args!("discovering legacy job metadata {}: {error}", path.display()));
                        continue;
                    }
                };
                for kind in SCALARS {
                    if disk::exists(path.with_extension(kind))? {
                        prepare(job.scalar(kind));
                    }
                }
                prepare(crate::run_facts::writing::prepare(&path).map(|_| ()));
                prepare(crate::run_facts::visits::prepare(&path).map(|_| ()));
                prepare(crate::scratch::jobs::prepare(&path).map(|_| ()));
                prepare(crate::job_receipts::storage::prepare(&path).map(|_| ()));
            }
        }
    }
    Ok(ready)
}
fn hidden(path: &Path) -> bool {
    path.file_name()
        .is_none_or(|name| name.to_string_lossy().starts_with('.') || name.to_string_lossy().ends_with(".lock"))
}
fn entries(path: &Path) -> io::Result<Vec<std::path::PathBuf>> {
    match disk::read_dir(path) {
        Ok(entries) => entries.map(|entry| entry.map(|entry| entry.path())).collect(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error),
    }
}
