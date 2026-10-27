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
use fastctx_types::layout::ConversationDir;
use fastctx_types::residents::{Notice, Owing, Pending, PendingState};
use fastctx_types::run::JobRecord;
use fastctx_types::{JobId, MachineName, NoticeId};

use crate::Node;
use crate::speech::{Owed, tell};

/// Keeps something on its way into the resident's session. Returns `false`
/// when it is not new: it is kept already, or went in before.
pub fn keep(directory: &ConversationDir, pending: &Pending) -> io::Result<bool> {
    let _transaction = crate::resident_storage::lock();
    if delivered_here(directory, &pending.delivery.id)?
        || directory.uncertain().join(format!("{}.json", pending.delivery.id)).exists()
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

/// Whether durable business custody already exists under this delivery id.
pub(crate) fn knows(directory: &ConversationDir, id: &NoticeId) -> io::Result<bool> {
    let _transaction = crate::resident_storage::lock();
    Ok(delivered_here(directory, id)?
        || directory.book("host-refusals").join(format!("{id}.json")).try_exists()?
        || path(directory, id).try_exists()?
        || directory.uncertain().join(format!("{id}.json")).try_exists()?)
}

/// Retains words whose host acceptance cannot be established.
pub fn retain_uncertain(directory: &ConversationDir, pending: &Pending) -> io::Result<()> {
    let _transaction = crate::resident_storage::lock();
    let uncertain = directory.uncertain();
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

/// Words judged not known to have gone in that their host now says it took:
/// they are kept again, on their way, for the turn that took them to settle
/// as any words do (`specs/world-concurrency.md`, 「不知是否送到」是暂时的判断).
pub fn reclaim(directory: &ConversationDir, ids: &[String]) -> io::Result<()> {
    let _transaction = crate::resident_storage::lock();
    for id in ids {
        let Ok(id) = NoticeId::try_from(id.as_str()) else { continue };
        let uncertain = directory.uncertain().join(format!("{id}.json"));
        let bytes = match std::fs::read(&uncertain) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        if delivered_here(directory, &id)? {
            continue;
        }
        ensure_private_dir(&directory.inbox())?;
        let kept = path(directory, &id);
        if !kept.exists() {
            publish(&kept, &bytes, Permissions::Private)?;
        }
        std::fs::remove_file(&uncertain)?;
    }
    Ok(())
}

/// Returns a definitely refused composer input to its own queue, before any host accepted it.
pub fn withdraw(directory: &ConversationDir, id: &NoticeId) -> io::Result<()> {
    let _transaction = crate::resident_storage::lock();
    let Some(pending) = read_here(directory, id)? else { return Ok(()) };
    if pending.state != PendingState::Waiting {
        return Err(io::Error::other("cannot withdraw an input crossing the host boundary"));
    }
    std::fs::remove_file(path(directory, id))
}

/// Records how far something kept has come.
pub fn set_state(directory: &ConversationDir, id: &NoticeId, state: PendingState) -> io::Result<()> {
    let _transaction = crate::resident_storage::lock();
    let Some(mut pending) = read_here(directory, id)? else { return Ok(()) };
    pending.state = state;
    // Which host activation has them is told by when they went to it.
    if matches!(state, PendingState::Handed | PendingState::Waking) {
        pending.since = Some(jiff::Timestamp::now());
    }
    publish(&path(directory, id), &json(&pending)?, Permissions::Private)
}

pub fn target(directory: &ConversationDir, id: &NoticeId, session: &str) -> io::Result<()> {
    let _transaction = crate::resident_storage::lock();
    let Some(mut pending) = read_here(directory, id)? else { return Ok(()) };
    if pending.session.as_deref().is_some_and(|prior| prior != session) {
        return Err(io::Error::other("these words belong to the prior conversation"));
    }
    pending.session = Some(session.to_string());
    publish(&path(directory, id), &json(&pending)?, Permissions::Private)
}

/// Everything on its way into the resident's session, oldest first.
pub fn pending(directory: &ConversationDir) -> io::Result<Vec<Pending>> {
    let _transaction = crate::resident_storage::lock();
    pending_here(directory)
}

/// Reads both input states atomically: reconciliation can move uncertain words back to pending.
pub(crate) fn input_states(directory: &ConversationDir) -> io::Result<(Vec<Pending>, Vec<Pending>)> {
    let _transaction = crate::resident_storage::lock();
    let pending = pending_here(directory)?;
    let mut uncertain = Vec::new();
    for path in entries(&directory.uncertain())? {
        if path.file_name().is_some_and(|name| name.to_string_lossy().starts_with('.'))
            || path.extension().is_none_or(|extension| extension != "json")
        {
            continue;
        }
        uncertain.push(serde_json::from_slice(&std::fs::read(path)?)?);
    }
    Ok((pending, uncertain))
}

/// The caller holds the resident-storage transaction.
pub(crate) fn pending_here(directory: &ConversationDir) -> io::Result<Vec<Pending>> {
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
            && !directory.uncertain().join(format!("{}.json", pending.delivery.id)).exists()
        {
            all.push(pending);
        }
    }
    all.sort_by(|a: &Pending, b| a.delivery.at.cmp(&b.delivery.at).then(a.delivery.id.cmp(&b.delivery.id)));
    Ok(all)
}

/// Something kept that went in: it leaves the inbox and goes on the
/// `delivered` list. Returns it, or `None` when it was settled already.
pub fn settle(node: &Node, directory: &ConversationDir, id: &NoticeId) -> io::Result<Option<Pending>> {
    // A command's end is confirmed to its job as it leaves the inbox, under
    // the disk's lock, which comes before the records': found to be one, the
    // records are let go and taken again after it.
    let mut gate = None;
    loop {
        let transaction = crate::resident_storage::lock();
        let Some(pending) = read_here(directory, id)? else { return Ok(None) };
        if gate.is_none() && matches!(pending.delivery.notice, Notice::JobEnded { .. }) {
            drop(transaction);
            gate = Some(node.disk.lock());
            continue;
        }
        return settle_read(node, directory, id, pending, gate.as_ref());
    }
}

/// `settle` of `pending`, read under the records' lock its caller holds,
/// with the disk's lock held too when it is a command's end.
fn settle_read(
    node: &Node,
    directory: &ConversationDir,
    id: &NoticeId,
    pending: Pending,
    gate: Option<&fastctx_platform::doing::SyncMutexGuard<'_, ()>>,
) -> io::Result<Option<Pending>> {
    if delivered_here(directory, id)? {
        if crate::residents::read_removed(directory)?.is_none() {
            std::fs::remove_file(path(directory, id))?;
        }
        return Ok(None);
    }
    let removed = crate::residents::read_removed(directory)?.is_some();
    if let Notice::JobEnded { job, machine, output, .. } = &pending.delivery.notice {
        let gate = gate.ok_or_else(|| io::Error::other("a command's end settled without the disk's lock"))?;
        let about: fastctx_types::residents::About = parse(&directory.about(), &std::fs::read(directory.about())?)?;
        crate::retention::confirmed_under(
            gate,
            node,
            fastctx_types::Address { machine: node.machine(), resident: about.id },
            machine.clone(),
            fastctx_types::run::JobDelivery {
                job: job.clone(),
                output: output.clone(),
                at: jiff::Timestamp::now(),
                shown_whole: false,
            },
            Some(&about.acting),
        )?;
    }
    let receipts = directory.delivery_receipts();
    ensure_private_dir(&receipts)?;
    publish(&receipts.join(id.as_str()), b"", Permissions::Private)?;
    // What went into a removed conversation stays in its inbox: its records
    // are what is left of it.
    if removed {
        return Ok(Some(pending));
    }
    match std::fs::remove_file(path(directory, id)) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
        _ => {}
    }
    Ok(Some(pending))
}

/// Whether something with this id went into the resident's session.
pub(crate) fn delivered_here(directory: &ConversationDir, id: &NoticeId) -> io::Result<bool> {
    if directory.delivery_receipts().join(id.as_str()).try_exists()? {
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

/// A wait took this completion, including a copy already queued in the inbox.
pub(crate) fn consume(directory: &ConversationDir, id: &NoticeId) -> io::Result<()> {
    let _transaction = crate::resident_storage::lock();
    let receipts = directory.delivery_receipts();
    ensure_private_dir(&receipts)?;
    match publish_new(&receipts.join(id.as_str()), b"consumed", Permissions::Private) {
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        result => result?,
    }
    if crate::residents::read_removed(directory)?.is_some() {
        return Ok(());
    }
    match std::fs::remove_file(path(directory, id)) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

fn read_here(directory: &ConversationDir, id: &NoticeId) -> io::Result<Option<Pending>> {
    let path = path(directory, id);
    match std::fs::read(&path) {
        Ok(bytes) => parse_pending(directory, &path, &bytes).map(Some),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn path(directory: &ConversationDir, id: &NoticeId) -> std::path::PathBuf {
    directory.inbox().join(format!("{id}.json"))
}

/// Keeps a turn's end for one reader until the reader's machine has it.
pub fn owe(directory: &ConversationDir, owing: &Owing) -> io::Result<()> {
    let _transaction = crate::resident_storage::lock();
    ensure_private_dir(&directory.outbox())?;
    if paid_here(directory, &owing.delivery.id)? {
        return Ok(());
    }
    let path = directory.outbox().join(format!("{}.json", owing.delivery.id));
    match publish_new(&path, &json(owing)?, Permissions::Private) {
        Err(error) if error.kind() != io::ErrorKind::AlreadyExists => Err(error),
        _ => Ok(()),
    }
}

/// The turns' ends still on their way, oldest first.
pub fn owing(directory: &ConversationDir) -> io::Result<Vec<Owing>> {
    let _transaction = crate::resident_storage::lock();
    let mut all = Vec::new();
    for path in entries(&directory.outbox())? {
        let hidden = path.file_name().is_some_and(|name| name.to_string_lossy().starts_with('.'));
        if hidden || path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        let owing: Owing = parse(&path, &std::fs::read(&path)?)?;
        if !paid_here(directory, &owing.delivery.id)? {
            all.push(owing);
        }
    }
    all.sort_by(|a: &Owing, b| a.delivery.at.cmp(&b.delivery.at).then(a.delivery.id.cmp(&b.delivery.id)));
    Ok(all)
}

/// A turn's end reached its reader's machine.
pub fn paid(directory: &ConversationDir, id: &NoticeId) -> io::Result<()> {
    let _transaction = crate::resident_storage::lock();
    ensure_private_dir(&directory.paid())?;
    publish(&directory.paid().join(id.as_str()), b"", Permissions::Private)?;
    match std::fs::remove_file(directory.outbox().join(format!("{id}.json"))) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

/// Whether a turn's end reached its reader's machine; older nodes marked it
/// beside the answer.
fn paid_here(directory: &ConversationDir, id: &NoticeId) -> io::Result<bool> {
    Ok(directory.paid().join(id.as_str()).try_exists()?
        || directory.book("undeliverable").join(id.as_str()).try_exists()?
        || directory.outbox().join(format!("{id}.paid")).try_exists()?)
}

/// A local recipient whose home is gone can never consume this answer.
/// Preserve that disposition separately from successful delivery receipts.
pub(crate) fn without_reader(directory: &ConversationDir, owing: &Owing) -> io::Result<()> {
    let _transaction = crate::resident_storage::lock();
    let path = directory.book("undeliverable").join(owing.delivery.id.as_str());
    ensure_private_dir(path.parent().expect("receipt directory"))?;
    publish(&path, &json(&owing.to)?, Permissions::Private)?;
    fastctx_platform::owned_directory::delete(&directory.outbox().join(format!("{}.json", owing.delivery.id)))
}

fn parse_pending(directory: &ConversationDir, path: &Path, bytes: &[u8]) -> io::Result<Pending> {
    let value: serde_json::Value = parse(path, bytes)?;
    if value.get("delivery").is_some() {
        return serde_json::from_value(value).map_err(io::Error::other);
    }
    // Before durable handovers the inbox contained Delivery directly.
    let delivery: fastctx_types::residents::Delivery = serde_json::from_value(value).map_err(io::Error::other)?;
    let (readers, working) = match &delivery.notice {
        Notice::Question { .. }
        | Notice::Words { goal_report: Some(_), .. }
        | Notice::Words { goal_question: Some(_), .. } => (Vec::new(), false),
        Notice::Words { from: fastctx_types::session::Speaker::Resident { address }, .. } => {
            (vec![address.clone()], false)
        }
        Notice::Words { from: fastctx_types::session::Speaker::User { .. }, .. } | Notice::Sentinel { .. } => {
            (Vec::new(), false)
        }
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
    let pending = Pending { delivery, readers, state, working, session: None, since: None };
    publish(&self::path(directory, &pending.delivery.id), &json(&pending)?, Permissions::Private)?;
    Ok(pending)
}

/// Marks that the resident who started `job` is owed word of its end.
/// Returns the job's record as it stands after the marker is there: if it
/// has ended by then, nobody else will tell, and the caller does.
pub fn owe_job(record_path: &Path) -> io::Result<JobRecord> {
    match publish_new(&record_path.with_extension("owed"), b"", Permissions::Private) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            if std::fs::read(record_path.with_extension("owed"))? == b"answer" {
                publish(&record_path.with_extension("owed"), b"", Permissions::Private)?;
            }
        }
        Err(error) => return Err(error),
    }
    record::read(record_path)
}

/// Tells the resident that started the job of `record` that it ended, if it
/// has and the resident is owed that word.
pub async fn tell_if_ended(node: &Arc<Node>, record_path: &Path, record: &JobRecord) -> io::Result<()> {
    if record.is_sentinel() {
        if record.end.is_none() {
            crate::job_watch::restore(node, record_path, record)?;
        }
        return Ok(());
    }
    let Some(owner) = record.owner.resident() else { return Ok(()) };
    let Some(end) = &record.end else {
        crate::job_watch::restore(node, record_path, record)?;
        return Ok(());
    };
    let owed = record_path.with_extension("owed");
    let _delivery = node.job_ends.lock(record_path).await;
    if record_path.with_extension("wait-taken").try_exists()? {
        match std::fs::remove_file(&owed) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
            _ => return Ok(()),
        }
    }
    // An `on` job's follower takes its end; a letter goes only when nobody
    // follows it then (`on`).
    if !owed.try_exists()? || !crate::on::unfollowed(node, record_path) {
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
            crate::job_watch::copied(node, record_path);
        }
        return Ok(());
    }
    if record_path.with_extension("delivered").try_exists()? {
        return crate::completion_debt::paid(record_path);
    }
    let account = record.on.as_ref().and_then(|on| on.account.clone()).or_else(|| record.account.clone());
    let acting =
        account.map(|account| fastctx_types::access::Acting::one(account.hub, account.account)).unwrap_or_default();
    if let Some(reason) = crate::obligations::impossible(node, &acting, &(&owner.machine).into())? {
        let bytes = std::fs::read(&owed)?;
        crate::obligations::finish(node, &owed, &bytes, crate::obligations::Outcome::Impossible, &reason)?;
        return Ok(());
    }
    if !crate::completion_debt::ready(node, record_path, owner)? {
        return Err(crate::completion_debt::pending("the host has not yet acknowledged the run answer"));
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
            taken: false,
            job: record.id.clone(),
            machine: record.machine.clone(),
            request: record.on.as_ref().and_then(|on| on.request.clone()),
            follower: record.on.as_ref().and_then(|on| on.follower.clone()),
            consumed: crate::job_receipts::snapshot(node, record_path)?,
            follower_unconfirmed: false,
            how,
            status: record.exit.map(|exit| exit.status),
            at: end.at,
            output: record.output.clone(),
            stderr: record.stderr.clone(),
            last: fastctx_hands::run::last::last_streams(
                std::path::Path::new(&record.output),
                record.stderr.as_deref().map(Path::new),
            )?,
            stop: record.stop.as_ref().map(|stop| Box::new(stop.request.clone())),
            scratch: crate::scratch::jobs::locations(record_path, &record.machine)?,
        };
        match publish_new(&notice_path, &json(&notice)?, Permissions::Private) {
            Err(error) if error.kind() != io::ErrorKind::AlreadyExists => return Err(error),
            _ => {}
        }
        // Concurrent notification attempts always send the same frozen
        // content, including its tail, under the stable letter id.
        parse(&notice_path, &std::fs::read(&notice_path)?)?
    };
    let id = job_notice_id(record);
    // The end is news for the account the job was sent for, or run for.
    let acting = record
        .on
        .as_ref()
        .and_then(|on| on.account.clone())
        .or_else(|| record.account.clone())
        .map(|account| fastctx_types::access::Acting::one(account.hub, account.account))
        .unwrap_or_default();
    if node.is_me(&owner.machine) {
        tell(node, &acting, owner, id, notice, Owed::Working)
            .await
            .map_err(|failure| io::Error::other(failure.message))?;
    } else if let Err(reason) = crate::completion_debt::post(node, record_path, owner, &acting, id, notice)? {
        let bytes = std::fs::read(&owed)?;
        crate::obligations::finish(node, &owed, &bytes, crate::obligations::Outcome::Impossible, &reason)?;
        return Ok(());
    }
    crate::completion_debt::paid(record_path)
}

pub(crate) fn job_notice_id(record: &JobRecord) -> NoticeId {
    let owner = record.owner.resident().expect("a completion has a resident owner");
    end_id(owner, &record.machine, &record.id, record.on.as_ref().and_then(|on| on.request.as_ref()))
}

pub(crate) fn end_id(
    owner: &fastctx_types::Address,
    machine: &MachineName,
    job: &JobId,
    request: Option<&NoticeId>,
) -> NoticeId {
    if let Some(request) = request {
        // Short job ids are reserved within one owner's town directory.
        // The request distinguishes jobs even after that owner changes towns.
        crate::speech::fixed_id(
            "on-end",
            &[machine.key(), owner.machine.key(), owner.resident.as_str(), job.as_str(), request.as_str()],
        )
    } else {
        // An older job may already have posted this id before an upgrade.
        // Only names of lowercase English letters, digits and hyphens made
        // such ids; any other name, or one too long for an id, is hashed.
        NoticeId::try_from(format!("{}-{job}", machine.key()))
            .unwrap_or_else(|_| crate::speech::fixed_id("end", &[machine.key(), job.as_str()]))
    }
}

/// Finishes telling the ends of jobs a node that stopped left owed.
pub async fn settle_jobs(node: &Arc<Node>) -> io::Result<Vec<(std::path::PathBuf, io::Error)>> {
    let mut problems = Vec::new();
    let mut owed = Vec::new();
    for parent in node.world.job_parents()? {
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
            Ok(job) => tell_if_ended(node, &path.with_extension("json"), &job).await,
            Err(error) => Err(error),
        };
        if let Err(error) = told {
            retry_job(node, &path.with_extension("json"));
            if error.kind() != io::ErrorKind::WouldBlock {
                problems.push((path, error));
            }
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
        let mut again = crate::again::Again::default();
        let mut ways = node.ways.subscribe();
        loop {
            ways.borrow_and_update();
            let result = match record::read(&path) {
                Ok(record) => tell_if_ended(&node, &path, &record).await,
                Err(error) if error.kind() == io::ErrorKind::NotFound => break,
                Err(error) => Err(error),
            };
            match result {
                Ok(()) => break,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                Err(error) => {
                    if let Err(recording) =
                        crate::obligations::retry(&node, &path.with_extension("owed"), &error.to_string())
                    {
                        node.log.line(format_args!("recording the completion retry reason: {recording}"));
                    }
                }
            }
            let pause = crate::completion_debt::pause(&path).ok().flatten().unwrap_or_else(|| again.next());
            tokio::select! {
                () = tokio::time::sleep(pause.min(fastctx_net::again::LONGEST)) => {}
                _ = ways.changed() => {}
            }
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

pub fn read(directory: &ConversationDir, id: &NoticeId) -> io::Result<Option<Pending>> {
    let _transaction = crate::resident_storage::lock();
    read_here(directory, id)
}
