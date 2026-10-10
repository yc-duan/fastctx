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

use std::io::{self, BufRead as _};
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

/// Returns a definitely refused composer input to its own queue, before any host accepted it.
pub fn withdraw(directory: &ResidentDir, id: &NoticeId) -> io::Result<()> {
    let (_transaction, directory) = crate::resident_storage::current(directory)?;
    let Some(pending) = read_here(&directory, id)? else { return Ok(()) };
    if pending.state != PendingState::Waiting {
        return Err(io::Error::other("cannot withdraw an input crossing the host boundary"));
    }
    std::fs::remove_file(path(&directory, id))
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
    let receipts = directory.path().join("delivery-receipts");
    ensure_private_dir(&receipts)?;
    publish(&receipts.join(id.as_str()), b"", Permissions::Private)?;
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
    if directory.path().join("delivery-receipts").join(id.as_str()).try_exists()? {
        return Ok(true);
    }
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
    let directory = world.job_owner(here, job_record.town.as_ref(), &job_record.owner);
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
    let Some(owner) = record.owner.resident() else { return Ok(()) };
    let Some(end) = &record.end else {
        crate::job_watch::restore(node, record)?;
        return Ok(());
    };
    let owed = node.world.job_owner(&node.machine(), record.town.as_ref(), &record.owner).job_owed(&record.id);
    let record_path = owed.with_extension("json");
    // An `on` job's follower takes its end; a letter goes only when nobody
    // follows it then (`on`).
    if !owed.try_exists()? || !crate::on::unfollowed(node, &record_path) {
        return Ok(());
    }
    let cancelled = record
        .on
        .as_ref()
        .and_then(|on| on.request.as_ref())
        .map(|request| {
            record_path.parent().expect("jobs directory").join(format!("request-{request}.stopped")).try_exists()
        })
        .transpose()?
        .unwrap_or(false);
    let taken = record_path.with_extension("on-taken").try_exists()?;
    if taken || record_path.with_extension("on-stopped").try_exists()? || cancelled {
        match std::fs::remove_file(&owed) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
            _ => {}
        }
        // Whoever followed it took every byte and its end, so they are
        // part of the hop that followed it now: one copy of an output stays,
        // there (`world-run.md`, 输出).
        // Output that went straight on goes once it got there too.
        let piped = record.on.as_ref().is_some_and(|on| on.piped);
        if taken && (!piped || record_path.with_extension("on-piped").try_exists()?) {
            crate::job_watch::forget(node, &record_path);
        }
        return Ok(());
    }
    let notice_path = record_path.with_extension("notice");
    let notice = if notice_path.try_exists()? {
        parse(&notice_path, &std::fs::read(&notice_path)?)?
    } else {
        let loss = record
            .output_lost
            .as_ref()
            .map(|loss| ("stdout", loss))
            .or_else(|| record.stderr_lost.as_ref().map(|loss| ("stderr", loss)));
        let how = match loss {
            Some((stream, loss)) => fastctx_types::run::EndedHow::Failed {
                error: format!(
                    "{stream} stopped being kept after {} bytes: {}; only the saved output is available",
                    loss.kept_bytes, loss.error
                ),
            },
            None => end.how.clone(),
        };
        let notice = Notice::JobEnded {
            job: record.id.clone(),
            machine: record.machine.clone(),
            request: record.on.as_ref().and_then(|on| on.request.clone()),
            follower: record.on.as_ref().and_then(|on| on.follower.clone()),
            how,
            status: record.exit.map(|exit| exit.status),
            at: end.at,
            output: record.output.clone(),
            stderr: record.stderr.clone(),
            last: fastctx_hands::run::last::last_streams(
                std::path::Path::new(&record.output),
                record.stderr.as_deref().map(Path::new),
            )?,
            stop: record.stop.as_ref().map(|stop| stop.request.clone()),
        };
        match publish_new(&notice_path, &json(&notice)?, Permissions::Private) {
            Err(error) if error.kind() != io::ErrorKind::AlreadyExists => return Err(error),
            _ => {}
        }
        // Concurrent notification attempts always send the same frozen
        // content, including its tail, under the stable letter id.
        parse(&notice_path, &std::fs::read(&notice_path)?)?
    };
    let id = if let Some(request) = record.on.as_ref().and_then(|on| on.request.as_ref()) {
        // Short job ids are reserved within one owner's town directory.
        // The request distinguishes jobs even after that owner changes towns.
        crate::speech::fixed_id(
            "on-end",
            &[
                record.machine.as_str(),
                owner.machine.as_str(),
                owner.resident.as_str(),
                record.id.as_str(),
                request.as_str(),
            ],
        )
    } else {
        // An older job may already have posted this id before an upgrade.
        NoticeId::try_from(format!("{}-{}", record.machine, record.id)).expect("a machine and a job make an id")
    };
    // The end is news for the account the job was sent for, or run for.
    let acting = record
        .on
        .as_ref()
        .and_then(|on| on.account.clone())
        .or_else(|| record.account.clone())
        .map(|account| fastctx_types::access::Acting::one(account.hub, account.account))
        .unwrap_or_default();
    tell(node, &acting, owner, id, notice, Owed::Working).await.map_err(|failure| io::Error::other(failure.message))?;
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
        let residents = match entries(&parent) {
            Ok(entries) => entries,
            Err(error) => {
                problems.push((parent, error));
                continue;
            }
        };
        for resident in residents {
            let jobs = resident.join("jobs");
            let records = match entries(&jobs) {
                Ok(entries) => entries,
                Err(error) => {
                    problems.push((jobs, error));
                    continue;
                }
            };
            for path in records {
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
            retry_job(node, &path.with_extension("json"));
            problems.push((path, error));
        }
    }
    Ok(problems)
}

/// A failed durable handover remains owed. Retry independently of the
/// supervisor, which may already have exited after its final notification.
pub(crate) fn retry_job(node: &Arc<Node>, path: &Path) {
    if !node.notifying_jobs.lock().expect("never poisoned").insert(path.to_path_buf()) {
        return;
    }
    let (node, path) = (node.clone(), path.to_path_buf());
    tokio::spawn(async move {
        let mut delay = crate::on::Backoff::default();
        loop {
            let result = match record::read(&path) {
                Ok(record) => tell_if_ended(&node, &record).await,
                Err(error) => Err(error),
            };
            match result {
                Ok(()) => break,
                Err(error) => node.log.line(format_args!("{} still owes its completion: {error}", path.display())),
            }
            tokio::time::sleep(delay.next()).await;
        }
        node.notifying_jobs.lock().expect("never poisoned").remove(&path);
    });
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
