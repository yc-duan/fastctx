//! Discovery names only node-owned records. It never interprets arbitrary files
//! in a conversation, page, attachment directory or job output as SQL metadata.
use crate::Node;
use fastctx_platform::disk;
use fastctx_types::layout::{Kept, Kind};
use std::{
    collections::VecDeque,
    io,
    path::{Path, PathBuf},
    sync::Arc,
};

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

// Each pass admits bounded work and remembers sources already ready. New node
// writes use SQL; gathering and bundle installation happen before registration.
#[derive(Default)]
pub(super) struct Catalog {
    rooms: Option<VecDeque<Kept>>,
    jobs: Option<VecDeque<PathBuf>>,
    shared: bool,
}
impl Catalog {
    pub(super) fn scan(&mut self, node: &Arc<Node>) -> io::Result<bool> {
        let transfers = crate::conversation_store::transfer::scan(node)?;
        if self.rooms.is_none() {
            self.rooms = Some(node.world.kept()?.into());
        }
        let rooms = self.rooms.as_mut().expect("discovered rooms");
        for _ in 0..rooms.len().min(128) {
            let kept = rooms.pop_front().expect("pending room");
            fastctx_platform::readings::counter("migration.discovery_rooms").add(1);
            if !completed(node, room(node, &kept)) {
                rooms.push_back(kept);
            }
        }
        if !self.shared {
            self.shared = completed(node, shared(node));
        }
        if self.jobs.is_none() {
            match inventory(node) {
                Ok(jobs) => self.jobs = Some(jobs),
                Err(error) => {
                    completed(node, Err(error));
                    return Ok(false);
                }
            }
        }
        let jobs = self.jobs.as_mut().expect("discovered jobs");
        for _ in 0..jobs.len().min(128) {
            let path = jobs.pop_front().expect("pending job");
            fastctx_platform::readings::counter("migration.discovery_jobs").add(1);
            if !completed(node, job(node, &path)) {
                jobs.push_back(path);
            }
        }
        fastctx_platform::readings::gauge("migration.discovery_remaining")
            .set((rooms.len() + jobs.len() + usize::from(!self.shared)) as i64);
        Ok(transfers && rooms.is_empty() && jobs.is_empty() && self.shared)
    }
}
fn completed(node: &Node, result: io::Result<bool>) -> bool {
    match result {
        Ok(ready) => ready,
        Err(error) => {
            if error.kind() != io::ErrorKind::WouldBlock {
                node.log.line(format_args!("discovering legacy records: {error}"));
            }
            false
        }
    }
}
fn prepare(node: &Node, work: impl FnOnce(&mut dyn FnMut(io::Result<()>)) -> io::Result<()>) -> io::Result<bool> {
    let mut ready = true;
    work(&mut |result| {
        if let Err(error) = result {
            ready = false;
            if error.kind() != io::ErrorKind::WouldBlock {
                node.log.line(format_args!("migrating legacy records: {error}"));
            }
        }
    })?;
    Ok(ready)
}
fn room(node: &Arc<Node>, kept: &Kept) -> io::Result<bool> {
    let directory = &kept.directory;
    if crate::conversation_store::transfer::pending(node.world.path(), &kept.name) {
        return Ok(false);
    }
    if !disk::exists(directory.path())?
        || node
            .database
            .read_background_now(|db| crate::conversation_store::deleted(db, &kept.name))
            .map_err(io::Error::other)?
    {
        return Ok(true);
    }
    crate::record_database::get(directory)?;
    prepare(node, |prepare| {
        prepare(crate::gather::queues::prepare_conflicts(directory));
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
        prepare(crate::attachments::index::prepare(directory));
        prepare(crate::books::cleanup::prepare(directory));
        prepare(crate::collab::asked::prepare(directory));
        prepare(crate::collab::birth::prepare(directory));
        prepare(crate::context::input::stamps::prepare(directory));
        prepare(crate::focus::storage::prepare(directory));
        prepare(crate::here::records::prepare(directory));
        prepare(crate::history::context::heard::prepare(directory));
        prepare(crate::session_options::prepare(directory));
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
        Ok(())
    })
}
fn shared(node: &Arc<Node>) -> io::Result<bool> {
    prepare(node, |prepare| {
        for queue in QUEUES {
            prepare(crate::records::items::prepare(&node.world.path().join(queue).join(".items")).map(|_| ()));
        }
        let unclaimed = node.world.path().join("codex-reports");
        let (machine, _, _, _) = crate::record_database::item(&unclaimed.join(".reports"))?;
        let claiming = node.clone();
        prepare(crate::records::ensure_each(&machine, "unclaimed-host-reports", move |push| {
            for path in entries(&unclaimed)? {
                if path.extension().is_some_and(|extension| extension == "json" || extension == "jsonl")
                    && !crate::codex::report_store::claimed_legacy(&claiming, &path)
                {
                    super::file(&path, push, |_| {
                        Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "legacy host report could not be assigned to a conversation during layout gathering",
                        ))
                    })?;
                }
            }
            Ok(())
        }));
        prepare(crate::control_posts::requests::prepare(node).map(|_| ()));
        prepare(crate::attachments::index::prepare_all(node).map(|_| ()));
        prepare(crate::timers::store::Store::open(&node.world.path().join("timers")).map(|_| ()));
        Ok(())
    })
}
fn inventory(node: &Node) -> io::Result<VecDeque<PathBuf>> {
    let mut jobs = VecDeque::new();
    for parent in node.world.job_parents()? {
        for owner in entries(&parent)? {
            for path in entries(&owner.join("jobs"))? {
                if path.extension().is_some_and(|ext| ext == "json") && !hidden(&path) {
                    jobs.push_back(path);
                }
            }
        }
    }
    Ok(jobs)
}
fn job(node: &Node, path: &Path) -> io::Result<bool> {
    let Some(job) = super::jobs::open(node, path)? else {
        return Ok(true);
    };
    prepare(node, |prepare| {
        for kind in SCALARS {
            if disk::exists(path.with_extension(kind))? {
                prepare(job.scalar(kind));
            }
        }
        prepare(crate::run_facts::writing::prepare(path).map(|_| ()));
        prepare(crate::run_facts::visits::prepare(path).map(|_| ()));
        prepare(crate::scratch::jobs::prepare(path).map(|_| ()));
        prepare(crate::job_receipts::storage::prepare(path).map(|_| ()));
        Ok(())
    })
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
