//! What is on its way to a resident of this machine, and what a turn's end
//! owes its readers, kept on disk so that a node that stops loses neither.
//!
//! Something for a resident is kept in its `inbox` (`Pending`) from before
//! the World tries to put it into the session until it knows which turn
//! took it; whoever is owed that turn's end is owed it from then on. Its id
//! then goes on the resident's `delivered` list, and the same id arriving
//! again, as a letter sent again or an answer told again after a restart,
//! is known and not told twice.
//!
//! When a turn ends, an answer for each reader goes into the `outbox` of
//! the resident whose turn it was before the turn's debt is cleared, and
//! leaves it once the reader's machine has it. An answer's id comes from the
//! turn and the reader, so telling the same end again is the same answer.
//!
//! When a `run` call answers before its job has ended, the resident is owed
//! word of the end: a marker beside the job's record says so. When the job
//! ends, the word goes to the resident (`crate::speech::tell`), and only
//! then does the marker go; a node that stopped in between finds the marker
//! when it starts and finishes the telling.

use std::io::{self, BufRead as _, Write as _};
use std::path::Path;
use std::sync::Arc;

use fastctx_hands::run::record;
use fastctx_platform::{Permissions, ensure_private_dir, publish, publish_new};
use fastctx_types::layout::{ResidentDir, WorldDir};
use fastctx_types::residents::{Notice, Owing, Pending, PendingState};
use fastctx_types::run::JobRecord;
use fastctx_types::{JobId, MachineName, NoticeId};

use crate::Node;
use crate::speech::{Owed, tell};

/// Keeps something on its way into the resident's session. Returns `false`
/// when it is not new: it is kept already, or went in before.
pub fn keep(directory: &ResidentDir, pending: &Pending) -> io::Result<bool> {
    let (_transaction, current) = crate::resident_storage::current(directory)?;
    let directory = &current;
    if delivered_here(directory, &pending.delivery.id)?
        || directory.path().join("uncertain").join(format!("{}.json", pending.delivery.id)).exists()
    {
        return Ok(false);
    }
    ensure_private_dir(&directory.inbox())?;
    match publish_new(&path(directory, &pending.delivery.id), &json(pending)?, Permissions::Private) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(false),
        Err(error) => Err(error),
    }
}

/// Retains words whose host acceptance cannot be established.
pub fn retain_uncertain(directory: &ResidentDir, pending: &Pending) -> io::Result<()> {
    let (_transaction, current) = crate::resident_storage::current(directory)?;
    let directory = &current;
    let uncertain = directory.path().join("uncertain");
    ensure_private_dir(&uncertain)?;
    let path = uncertain.join(format!("{}.json", pending.delivery.id));
    match publish_new(&path, &json(pending)?, Permissions::Private) {
        Err(error) if error.kind() != io::ErrorKind::AlreadyExists => return Err(error),
        _ => {}
    }
    match std::fs::remove_file(directory.inbox().join(format!("{}.json", pending.delivery.id))) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

/// Records how far something kept has come.
pub fn set_state(directory: &ResidentDir, id: &NoticeId, state: PendingState) -> io::Result<()> {
    let (_transaction, current) = crate::resident_storage::current(directory)?;
    let directory = &current;
    let Some(mut pending) = read_here(directory, id)? else { return Ok(()) };
    pending.state = state;
    publish(&path(directory, id), &json(&pending)?, Permissions::Private)
}

pub fn target(directory: &ResidentDir, id: &NoticeId, session: &str) -> io::Result<()> {
    let (_transaction, current) = crate::resident_storage::current(directory)?;
    let directory = &current;
    let Some(mut pending) = read_here(directory, id)? else { return Ok(()) };
    if pending.session.as_deref().is_some_and(|prior| prior != session) {
        return Err(io::Error::other("these words belong to the prior conversation"));
    }
    pending.session = Some(session.to_string());
    publish(&path(directory, id), &json(&pending)?, Permissions::Private)
}

/// Everything on its way into the resident's session, oldest first.
pub fn pending(directory: &ResidentDir) -> io::Result<Vec<Pending>> {
    let (_transaction, current) = crate::resident_storage::current(directory)?;
    let directory = &current;
    let mut all = Vec::new();
    for path in entries(&directory.inbox())? {
        // A file being written sits under a hidden temporary name until it
        // is whole.
        let hidden = path.file_name().is_some_and(|name| name.to_string_lossy().starts_with('.'));
        if hidden || path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        let pending = parse_pending(directory, &path, &std::fs::read(&path)?)?;
        if !delivered_here(directory, &pending.delivery.id)?
            && !directory.path().join("uncertain").join(format!("{}.json", pending.delivery.id)).exists()
        {
            all.push(pending);
        }
    }
    all.sort_by(|a: &Pending, b| a.delivery.at.cmp(&b.delivery.at).then(a.delivery.id.cmp(&b.delivery.id)));
    Ok(all)
}

/// Something kept that went in: it leaves the inbox and goes on the
/// `delivered` list. Returns it, or `None` when it was settled already.
pub fn settle(directory: &ResidentDir, id: &NoticeId) -> io::Result<Option<Pending>> {
    let (_transaction, current) = crate::resident_storage::current(directory)?;
    let directory = &current;
    let Some(pending) = read_here(directory, id)? else { return Ok(None) };
    if delivered_here(directory, id)? {
        if directory.path().file_name().is_none_or(|name| name != "archive") {
            std::fs::remove_file(path(directory, id))?;
        }
        return Ok(None);
    }
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(directory.delivered())?;
    file.write_all(format!("{id}\n").as_bytes())?;
    if directory.path().file_name().is_some_and(|name| name == "archive") {
        return Ok(Some(pending));
    }
    match std::fs::remove_file(path(directory, id)) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
        _ => {}
    }
    Ok(Some(pending))
}

/// Whether something with this id went into the resident's session.
fn delivered_here(directory: &ResidentDir, id: &NoticeId) -> io::Result<bool> {
    let file = match std::fs::File::open(directory.delivered()) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    for line in io::BufReader::new(file).lines() {
        if line? == id.as_str() {
            return Ok(true);
        }
    }
    Ok(false)
}

fn read_here(directory: &ResidentDir, id: &NoticeId) -> io::Result<Option<Pending>> {
    let path = path(directory, id);
    match std::fs::read(&path) {
        Ok(bytes) => parse_pending(directory, &path, &bytes).map(Some),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn path(directory: &ResidentDir, id: &NoticeId) -> std::path::PathBuf {
    directory.inbox().join(format!("{id}.json"))
}

/// Keeps a turn's end for one reader until the reader's machine has it.
pub fn owe(directory: &ResidentDir, owing: &Owing) -> io::Result<()> {
    let (_transaction, current) = crate::resident_storage::current(directory)?;
    let directory = &current;
    ensure_private_dir(&directory.outbox())?;
    if directory.outbox().join(format!("{}.paid", owing.delivery.id)).exists() {
        return Ok(());
    }
    let path = directory.outbox().join(format!("{}.json", owing.delivery.id));
    match publish_new(&path, &json(owing)?, Permissions::Private) {
        Err(error) if error.kind() != io::ErrorKind::AlreadyExists => Err(error),
        _ => Ok(()),
    }
}

/// The turns' ends still on their way, oldest first.
pub fn owing(directory: &ResidentDir) -> io::Result<Vec<Owing>> {
    let (_transaction, current) = crate::resident_storage::current(directory)?;
    let directory = &current;
    let mut all = Vec::new();
    for path in entries(&directory.outbox())? {
        let hidden = path.file_name().is_some_and(|name| name.to_string_lossy().starts_with('.'));
        if hidden || path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        let owing: Owing = parse(&path, &std::fs::read(&path)?)?;
        if !directory.outbox().join(format!("{}.paid", owing.delivery.id)).exists() {
            all.push(owing);
        }
    }
    all.sort_by(|a: &Owing, b| a.delivery.at.cmp(&b.delivery.at).then(a.delivery.id.cmp(&b.delivery.id)));
    Ok(all)
}

/// A turn's end reached its reader's machine.
pub fn paid(directory: &ResidentDir, id: &NoticeId) -> io::Result<()> {
    let (_transaction, current) = crate::resident_storage::current(directory)?;
    let directory = &current;
    publish(&directory.outbox().join(format!("{id}.paid")), b"", Permissions::Private)?;
    if directory.path().file_name().is_some_and(|name| name == "archive") {
        return Ok(());
    }
    match std::fs::remove_file(directory.outbox().join(format!("{id}.json"))) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

fn parse_pending(directory: &ResidentDir, path: &Path, bytes: &[u8]) -> io::Result<Pending> {
    let value: serde_json::Value = parse(path, bytes)?;
    if value.get("delivery").is_some() {
        return serde_json::from_value(value).map_err(io::Error::other);
    }
    // Before durable handovers the inbox contained Delivery directly.
    let delivery: fastctx_types::residents::Delivery = serde_json::from_value(value).map_err(io::Error::other)?;
    let (readers, working) = match &delivery.notice {
        Notice::Words { from: fastctx_types::session::Speaker::Resident { address }, .. } => {
            (vec![address.clone()], false)
        }
        Notice::Words { from: fastctx_types::session::Speaker::User { .. }, .. } => (Vec::new(), false),
        _ => (Vec::new(), true),
    };
    let about: fastctx_types::residents::About = parse(&directory.about(), &std::fs::read(directory.about())?)?;
    // Old Codex inbox entries were written after any send error, including
    // an unanswered accepted request. They cannot safely be sent again.
    let state = if about.world_started && about.host.agent.as_str() == "codex" {
        PendingState::Sending
    } else {
        PendingState::Waiting
    };
    let pending = Pending { delivery, readers, state, working, session: None };
    publish(&self::path(directory, &pending.delivery.id), &json(&pending)?, Permissions::Private)?;
    Ok(pending)
}

/// Marks that the resident who started `job` is owed word of its end.
/// Returns the job's record as it stands after the marker is there: if it
/// has ended by then, nobody else will tell, and the caller does.
pub fn owe_job(world: &WorldDir, here: &MachineName, job: &JobId, record_path: &Path) -> io::Result<JobRecord> {
    let job_record = record::read(record_path)?;
    let directory = world.owner(here, job_record.town.as_ref(), &job_record.owner);
    match publish_new(&directory.job_owed(job), b"", Permissions::Private) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    record::read(record_path)
}

/// Tells the resident that started the job of `record` that it ended, if it
/// has and the resident is owed that word.
pub async fn tell_if_ended(node: &Arc<Node>, record: &JobRecord) -> io::Result<()> {
    let Some(end) = &record.end else {
        return Ok(());
    };
    let owed = node.world.owner(&node.machine(), record.town.as_ref(), &record.owner).job_owed(&record.id);
    if !owed.exists() {
        return Ok(());
    }
    let notice = Notice::JobEnded {
        job: record.id.clone(),
        machine: record.machine.clone(),
        how: end.how.clone(),
        status: record.exit.map(|exit| exit.status),
        at: end.at,
        output: record.output.clone(),
    };
    let id = NoticeId::try_from(format!("{}-{}", record.machine, record.id)).expect("a machine and a job make an id");
    tell(node, &record.owner, id, notice, Owed::Working).await.map_err(|failure| io::Error::other(failure.message))?;
    match std::fs::remove_file(&owed) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

/// Finishes telling the ends of jobs a node that stopped left owed.
pub async fn settle_jobs(node: &Arc<Node>) -> io::Result<Vec<(std::path::PathBuf, io::Error)>> {
    let mut problems = Vec::new();
    let mut owed = Vec::new();
    for parent in node.world.resident_parents()? {
        for resident in entries(&parent)? {
            for path in entries(&resident.join("jobs"))? {
                if path.extension().is_some_and(|extension| extension == "owed") {
                    owed.push(path);
                }
            }
        }
    }
    for path in owed {
        let told = match record::read(&path.with_extension("json")) {
            Ok(job) => tell_if_ended(node, &job).await,
            Err(error) => Err(error),
        };
        if let Err(error) = told {
            problems.push((path, error));
        }
    }
    Ok(problems)
}

fn entries(directory: &Path) -> io::Result<Vec<std::path::PathBuf>> {
    match std::fs::read_dir(directory) {
        Ok(entries) => entries.map(|entry| entry.map(|entry| entry.path())).collect(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error),
    }
}

fn parse<T: serde::de::DeserializeOwned>(path: &Path, bytes: &[u8]) -> io::Result<T> {
    serde_json::from_slice(bytes)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, format!("{}: {error}", path.display())))
}

fn json(value: &impl serde::Serialize) -> io::Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(io::Error::other)?;
    bytes.push(b'\n');
    Ok(bytes)
}

pub fn read(directory: &ResidentDir, id: &NoticeId) -> io::Result<Option<Pending>> {
    let (_transaction, directory) = crate::resident_storage::current(directory)?;
    read_here(&directory, id)
}
