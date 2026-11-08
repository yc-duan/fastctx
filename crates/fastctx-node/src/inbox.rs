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

use std::collections::HashSet;
use std::io::{self, BufRead as _};
use std::ops::ControlFlow;
use std::path::Path;
use std::sync::Arc;

use fastctx_hands::run::record;
use fastctx_types::layout::ConversationDir;
use fastctx_types::residents::{Notice, Owing, Pending, PendingState};
use fastctx_types::run::JobRecord;
use fastctx_types::{JobId, MachineName, NoticeId};

use crate::Node;
use crate::speech::{Owed, tell};

pub(crate) mod answers;
pub(crate) mod outgoing;
pub(crate) mod presence;
pub(crate) mod storage;
use storage::{Receipt, change, read_rows};

/// Keep an input once, before the host can observe it.
pub fn keep(directory: &ConversationDir, pending: &Pending) -> io::Result<bool> {
    crate::attachments::delivered::prepare(directory)?;
    let (pending, bytes) = (pending.clone(), serde_json::to_vec(pending)?.len());
    change(directory, bytes, move |rows| {
        let key = pending.delivery.id.as_str();
        if storage::delivered(rows, key)?
            || rows.get::<Pending>("uncertain-inputs", key)?.is_some()
            || rows.get::<Pending>("pending-inputs", key)?.is_some()
        {
            return Ok(false);
        }
        crate::attachments::delivered::record_in(rows, std::slice::from_ref(&pending))?;
        rows.put_ordered("pending-inputs", key, &pending, pending.delivery.at)?;
        Ok(true)
    })
}
pub(crate) fn knows(directory: &ConversationDir, id: &NoticeId) -> io::Result<bool> {
    if crate::lifecycle::refused::contains(directory, id)? {
        return Ok(true);
    }
    read_rows(directory, |rows| {
        Ok(storage::delivered(rows, id.as_str())?
            || rows.get::<Pending>("pending-inputs", id.as_str())?.is_some()
            || rows.get::<Pending>("uncertain-inputs", id.as_str())?.is_some())
    })
}
pub fn retain_uncertain(directory: &ConversationDir, pending: &Pending) -> io::Result<()> {
    let (pending, bytes) = (pending.clone(), serde_json::to_vec(pending)?.len());
    change(directory, bytes, move |rows| {
        let key = pending.delivery.id.as_str();
        if rows.get::<Pending>("uncertain-inputs", key)?.is_none() {
            rows.put("uncertain-inputs", key, &pending)?;
        }
        rows.delete("pending-inputs", key)?;
        Ok(())
    })
}
/// Which of `ids` still wait here, kept or in doubt: the only ones what the
/// host says of them can still settle. Read at once, however many.
pub fn waiting_among(directory: &ConversationDir, ids: &[String]) -> io::Result<HashSet<String>> {
    if ids.is_empty() {
        return Ok(HashSet::new());
    }
    read_rows(directory, |rows| {
        let mut waiting = HashSet::new();
        for id in ids {
            if rows.get::<Pending>("pending-inputs", id)?.is_some()
                || rows.get::<Pending>("uncertain-inputs", id)?.is_some()
            {
                waiting.insert(id.clone());
            }
        }
        Ok(waiting)
    })
}
pub fn reclaim(directory: &ConversationDir, ids: &[String]) -> io::Result<()> {
    for group in ids.chunks(16) {
        let ids = group.to_vec();
        change(directory, ids.iter().map(String::len).sum(), move |rows| {
            for id in ids {
                if NoticeId::try_from(id.as_str()).is_err() || storage::delivered(rows, &id)? {
                    continue;
                }
                if let Some(row) = rows.get::<Pending>("uncertain-inputs", &id)? {
                    if rows.get::<Pending>("pending-inputs", &id)?.is_none() {
                        rows.put_ordered("pending-inputs", &id, &row, row.delivery.at)?;
                    }
                    rows.delete("uncertain-inputs", &id)?;
                }
            }
            Ok(())
        })?;
    }
    Ok(())
}
pub fn withdraw(directory: &ConversationDir, id: &NoticeId) -> io::Result<()> {
    let key = id.to_string();
    change(directory, key.len(), move |rows| {
        if let Some(row) = rows.get::<Pending>("pending-inputs", &key)? {
            if row.state != PendingState::Waiting {
                return Err(fastctx_storage::Error::new("cannot withdraw an input crossing the host boundary"));
            }
            rows.delete("pending-inputs", &key)?;
        }
        Ok(())
    })
}
fn update(
    directory: &ConversationDir,
    id: &NoticeId,
    apply: impl FnOnce(&mut Pending) -> fastctx_storage::Result<()> + Send + 'static,
) -> io::Result<()> {
    let key = id.to_string();
    change(directory, 512, move |rows| {
        if let Some(mut row) = rows.get::<Pending>("pending-inputs", &key)? {
            apply(&mut row)?;
            rows.put_ordered("pending-inputs", &key, &row, row.delivery.at)?;
        }
        Ok(())
    })
}
pub fn set_state(directory: &ConversationDir, id: &NoticeId, state: PendingState) -> io::Result<()> {
    update(directory, id, move |row| {
        row.state = state;
        if matches!(state, PendingState::Handed | PendingState::Waking) {
            row.since = Some(jiff::Timestamp::now());
        }
        Ok(())
    })
}
pub fn target(directory: &ConversationDir, id: &NoticeId, session: &str) -> io::Result<()> {
    let session = session.to_string();
    update(directory, id, move |row| {
        if row.session.as_deref().is_some_and(|prior| !same_session(prior, &session)) {
            return Err(fastctx_storage::Error::new("these words belong to the prior conversation"));
        }
        if row.session.as_deref().is_none_or(|prior| prior.len() < session.len()) {
            row.session = Some(session);
        }
        Ok(())
    })
}
/// Claude's short session id is extended when its complete identity arrives.
pub(crate) fn same_session(bound: &str, session: &str) -> bool {
    let (short, long) = if bound.len() <= session.len() { (bound, session) } else { (session, bound) };
    bound == session || (short.len() == 8 && long.starts_with(short))
}
pub fn unbind(directory: &ConversationDir, id: &NoticeId) -> io::Result<()> {
    update(directory, id, |row| {
        row.session = None;
        Ok(())
    })
}
pub fn rebind(directory: &ConversationDir, id: &NoticeId, session: &str) -> io::Result<()> {
    let session = session.to_string();
    update(directory, id, move |row| {
        row.session = Some(session);
        Ok(())
    })
}
/// What waits, in the order it came, a page at a time. Each page is read on
/// its own, so `visit` may read and write records in between; it says when
/// it has seen enough. A long wait costs only as many pages as are looked at.
pub(crate) fn pages(
    directory: &ConversationDir,
    mut visit: impl FnMut(Vec<Pending>) -> io::Result<ControlFlow<()>>,
) -> io::Result<()> {
    storage::initialize(directory)?;
    let mut after = None;
    loop {
        let from = after.take();
        let (page, next) = crate::records::read(directory, move |rows| storage::pending_page(rows, from.as_ref()))?;
        if page.is_empty() || visit(page)?.is_break() {
            return Ok(());
        }
        match next {
            Some(next) => after = Some(next),
            None => return Ok(()),
        }
    }
}
/// The first of what waits that `matches`.
pub(crate) fn first(
    directory: &ConversationDir,
    mut matches: impl FnMut(&Pending) -> bool,
) -> io::Result<Option<Pending>> {
    let mut found = None;
    pages(directory, |page| {
        found = page.into_iter().find(|pending| matches(pending));
        Ok(if found.is_some() { ControlFlow::Break(()) } else { ControlFlow::Continue(()) })
    })?;
    Ok(found)
}
/// Whether anything waiting `matches`.
pub(crate) fn any(directory: &ConversationDir, matches: impl FnMut(&Pending) -> bool) -> io::Result<bool> {
    first(directory, matches).map(|found| found.is_some())
}
/// What waits that `matches`, for what only a few ever match: inputs in the
/// host's hands, being sent, or of one kind.
pub(crate) fn those(
    directory: &ConversationDir,
    mut matches: impl FnMut(&Pending) -> bool,
) -> io::Result<Vec<Pending>> {
    let mut found = Vec::new();
    pages(directory, |page| {
        found.extend(page.into_iter().filter(|pending| matches(pending)));
        Ok(ControlFlow::Continue(()))
    })?;
    Ok(found)
}
/// The inputs whose turn was not known to have taken them, in the order they
/// came, a page at a time like `pages`.
pub(crate) fn uncertain_pages(
    directory: &ConversationDir,
    mut visit: impl FnMut(Vec<Pending>) -> io::Result<ControlFlow<()>>,
) -> io::Result<()> {
    storage::initialize(directory)?;
    let mut after = 0;
    loop {
        let (page, next) = crate::records::read(directory, move |rows| {
            let page = rows.page::<Pending>("uncertain-inputs", after, 128)?;
            let next = page.last().map(|(position, _)| *position);
            Ok((page.into_iter().map(|(_, row)| row).collect::<Vec<_>>(), next))
        })?;
        if page.is_empty() || visit(page)?.is_break() {
            return Ok(());
        }
        match next {
            Some(next) => after = next,
            None => return Ok(()),
        }
    }
}
pub(crate) fn uncertain_lately(directory: &ConversationDir, within: std::time::Duration) -> io::Result<bool> {
    let now = jiff::Timestamp::now();
    let since = now.checked_sub(within).map_err(io::Error::other)?;
    read_rows(directory, |rows| rows.recent("uncertain-inputs", since, now))
}

/// Confirm an external job artifact first; receipt and inbox state commit together.
pub fn settle(node: &Node, directory: &ConversationDir, id: &NoticeId) -> io::Result<Option<Pending>> {
    let Some(pending) = read(directory, id)? else { return Ok(None) };
    let previously = delivered_here(directory, id)?;
    if !previously && let Notice::JobEnded { job, machine, output, .. } = &pending.delivery.notice {
        let about = crate::residents::read_about(directory)?;
        crate::retention::confirmed(
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
    let removed = crate::residents::read_removed(directory)?.is_some();
    let key = id.to_string();
    change(directory, 512, move |rows| {
        let current = rows.get::<Pending>("pending-inputs", &key)?;
        let already = previously
            || rows.get::<Receipt>("delivery-receipts", &key)?.is_some_and(|receipt| receipt != Receipt::Answered);
        if !already && current.is_some() {
            rows.put("delivery-receipts", &key, &Receipt::Heard)?;
        }
        if !removed {
            rows.delete("pending-inputs", &key)?;
        }
        Ok(if already { None } else { current })
    })
}
pub(crate) fn delivered_here(directory: &ConversationDir, id: &NoticeId) -> io::Result<bool> {
    read_rows(directory, |rows| storage::delivered(rows, id.as_str()))
}
pub(crate) fn consume(directory: &ConversationDir, id: &NoticeId) -> io::Result<()> {
    let removed = crate::residents::read_removed(directory)?.is_some();
    let key = id.to_string();
    change(directory, key.len(), move |rows| {
        if !storage::delivered(rows, &key)? {
            rows.put("delivery-receipts", &key, &Receipt::Consumed)?;
        }
        if !removed {
            rows.delete("pending-inputs", &key)?;
        }
        Ok(())
    })
}
pub(crate) fn heard_at(directory: &ConversationDir, id: &NoticeId) -> io::Result<Option<jiff::Timestamp>> {
    read_rows(directory, |rows| {
        if rows.get::<Receipt>("delivery-receipts", id.as_str())? == Some(Receipt::Heard) {
            rows.modified("delivery-receipts", id.as_str())
        } else {
            Ok(None)
        }
    })
}
pub fn owe(directory: &ConversationDir, owing: &Owing) -> io::Result<()> {
    let (owing, bytes) = (owing.clone(), serde_json::to_vec(owing)?.len());
    change(directory, bytes, move |rows| owe_in(rows, &owing))
}
/// Commit an outgoing answer with another record that creates its obligation.
pub(crate) fn owe_in(rows: &crate::records::Rows<'_>, owing: &Owing) -> fastctx_storage::Result<()> {
    let key = owing.delivery.id.as_str();
    if !storage::paid(rows, key)? && !rows.exists("outgoing-answers", key)? {
        rows.put_ordered("outgoing-answers", key, owing, owing.delivery.at)?;
    }
    Ok(())
}
/// Each answer owed and not yet paid, one at a time in the order they came.
pub(crate) fn each_owing(directory: &ConversationDir, mut visit: impl FnMut(Owing)) -> io::Result<()> {
    let mut after = answers::Cursor::default();
    while let Some((next, owing)) = answers::next(directory, &after)? {
        visit(owing);
        after = next;
    }
    Ok(())
}
pub fn paid(directory: &ConversationDir, id: &NoticeId) -> io::Result<()> {
    let key = id.to_string();
    change(directory, key.len(), move |rows| {
        outgoing::settled(rows, &key)?;
        rows.put("paid-answers", &key, &true)?;
        rows.delete("outgoing-answers", &key)?;
        Ok(())
    })
}
pub(crate) fn without_reader(directory: &ConversationDir, owing: &Owing) -> io::Result<()> {
    let (key, to) = (owing.delivery.id.to_string(), owing.to.clone());
    change(directory, key.len() + 128, move |rows| {
        outgoing::settled(rows, &key)?;
        rows.put("undeliverable-answers", &key, &to)?;
        rows.delete("outgoing-answers", &key)?;
        Ok(())
    })
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
    let about: fastctx_types::residents::About =
        parse(&directory.about(), &fastctx_platform::disk::read(directory.about())?)?;
    // Old Codex inbox entries were written after any send error, including
    // an unanswered accepted request. They cannot safely be sent again.
    let state = if about.world_started && about.host.agent.as_str() == "codex" {
        PendingState::Sending
    } else {
        PendingState::Waiting
    };
    let pending = Pending { delivery, readers, state, working, session: None, since: None };
    Ok(pending)
}

/// Marks that the resident who started `job` is owed word of its end.
/// Returns the job's record as it stands after the marker is there: if it
/// has ended by then, nobody else will tell, and the caller does.
pub fn owe_job(record_path: &Path) -> io::Result<JobRecord> {
    crate::job_metadata::put_raw(&record_path.with_extension("owed"), b"")?;
    record::read(record_path)
}

/// Tells the resident that started the job of `record` that it ended, if it
/// has and the resident is owed that word.
pub async fn tell_if_ended(node: &Arc<Node>, record_path: &Path, record: &JobRecord) -> io::Result<()> {
    crate::job_accounts::seen(node, record_path, record);
    let Some(owner) = record.owner.resident() else { return Ok(()) };
    if record.end.is_none() {
        crate::job_watch::restore(node, record_path, record).await?;
        return Ok(());
    };
    let _delivery = node.job_ends.lock(record_path).await;
    let (checking, path, ended) = (node.clone(), record_path.to_path_buf(), record.clone());
    let Some(notice) = fastctx_platform::task::spawn_blocking(move || prepare_job_end(&checking, &path, &ended))
        .await
        .map_err(io::Error::other)??
    else {
        return Ok(());
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
    } else {
        let (posting, path, owner) = (node.clone(), record_path.to_path_buf(), owner.clone());
        let posted = fastctx_platform::task::spawn_blocking(move || -> io::Result<bool> {
            if let Err(reason) = crate::completion_debt::post(&posting, &path, &owner, &acting, id, notice)? {
                let owed = path.with_extension("owed");
                let bytes = crate::job_metadata::read(&owed)?;
                crate::obligations::finish_job(
                    &posting,
                    &owed,
                    &bytes,
                    crate::obligations::Outcome::Impossible,
                    &reason,
                )?;
                return Ok(false);
            }
            Ok(true)
        })
        .await
        .map_err(io::Error::other)??;
        if !posted {
            return Ok(());
        }
    }
    let path = record_path.to_path_buf();
    fastctx_platform::task::spawn_blocking(move || crate::completion_debt::paid(&path))
        .await
        .map_err(io::Error::other)?
}

fn prepare_job_end(node: &Arc<Node>, record_path: &Path, record: &JobRecord) -> io::Result<Option<Notice>> {
    let owner = record.owner.resident().expect("resident completion");
    let end = record.end.as_ref().expect("ended record");
    let owed = record_path.with_extension("owed");
    if crate::job_metadata::exists(&record_path.with_extension("wait-taken"))? {
        match crate::job_metadata::delete(&owed) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
            _ => return Ok(None),
        }
    }
    // An `on` job's follower takes its end; a letter goes only when nobody
    // follows it then (`on`).
    if !crate::job_metadata::exists(&owed)? || !crate::on::unfollowed(node, record_path) {
        return Ok(None);
    }
    let cancelled = record
        .on
        .as_ref()
        .and_then(|on| on.request.as_ref())
        .map(|request| {
            fastctx_platform::disk::exists(
                record_path.parent().expect("jobs directory").join(format!("request-{request}.stopped")),
            )
        })
        .transpose()?
        .unwrap_or(false);
    let taken = crate::job_metadata::exists(&record_path.with_extension("on-taken"))?;
    if taken || crate::job_metadata::exists(&record_path.with_extension("on-stopped"))? || cancelled {
        // Whoever followed it took every byte and its end, so they are
        // part of the hop that followed it now: one copy of an output stays,
        // there (`world-run.md`, 输出).
        // Output that went straight on goes once it got there too.
        let piped = record.on.as_ref().is_some_and(|on| on.piped);
        if taken && (!piped || crate::job_metadata::exists(&record_path.with_extension("on-piped"))?) {
            crate::job_watch::copied(node, record_path)?;
        }
        match crate::job_metadata::delete(&owed) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
            _ => {}
        }
        return Ok(None);
    }
    if crate::job_metadata::exists(&record_path.with_extension("delivered"))? {
        return crate::completion_debt::paid(record_path).map(|_| None);
    }
    let account = record.on.as_ref().and_then(|on| on.account.clone()).or_else(|| record.account.clone());
    let acting =
        account.map(|account| fastctx_types::access::Acting::one(account.hub, account.account)).unwrap_or_default();
    if let Some(reason) = crate::obligations::impossible(node, &acting, &(&owner.machine).into())? {
        let bytes = crate::job_metadata::read(&owed)?;
        crate::obligations::finish_job(node, &owed, &bytes, crate::obligations::Outcome::Impossible, &reason)?;
        return Ok(None);
    }
    if !crate::completion_debt::ready(node, record_path, owner)? {
        return Err(crate::completion_debt::pending("the host has not yet acknowledged the run answer"));
    }
    let notice_path = record_path.with_extension("notice");
    let notice = if crate::job_metadata::exists(&notice_path)? {
        parse(&notice_path, &crate::job_metadata::read(&notice_path)?)?
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
        match crate::job_metadata::create(&notice_path, &json(&notice)?) {
            Err(error) if error.kind() != io::ErrorKind::AlreadyExists => return Err(error),
            _ => {}
        }
        // Concurrent notification attempts always send the same frozen
        // content, including its tail, under the stable letter id.
        parse(&notice_path, &crate::job_metadata::read(&notice_path)?)?
    };
    Ok(Some(notice))
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

/// Finishes telling the ends of jobs a node that stopped left owed. Looking
/// for what is owed is background work: a job whose records are still to
/// move in is queued behind what someone waits for, and answered as not
/// yet (`WouldBlock`), to be looked at again.
pub async fn settle_jobs(node: &Arc<Node>) -> io::Result<Vec<(std::path::PathBuf, io::Error)>> {
    let working = node.clone();
    let (mut problems, owed) = fastctx_platform::task::spawn_blocking(move || {
        crate::record_migration::scheduler::discovery(|| owing(&working))
    })
    .await
    .map_err(io::Error::other)??;
    for path in owed {
        let reading = path.with_extension("json");
        let read =
            fastctx_platform::task::spawn_blocking(move || record::read(&reading)).await.map_err(io::Error::other)?;
        let told = match read {
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

/// The jobs of every conversation here that owe the telling of their end,
/// and what could not be looked at.
type OwedJobs = (Vec<(std::path::PathBuf, io::Error)>, Vec<std::path::PathBuf>);
fn owing(node: &Node) -> io::Result<OwedJobs> {
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
                if path.extension().is_some_and(|extension| extension == "json") {
                    match crate::job_metadata::exists(&path.with_extension("owed")) {
                        Ok(true) => owed.push(path),
                        Ok(false) => {}
                        Err(error) => problems.push((path, error)),
                    }
                }
            }
        }
    }
    Ok((problems, owed))
}

/// A failed durable handover remains owed. Retry independently of the
/// supervisor, which may already have exited after its final notification.
pub(crate) fn retry_job(node: &Arc<Node>, path: &Path) {
    if !node.notifying_jobs.lock().expect("never poisoned").insert(path.to_path_buf()) {
        return;
    }
    let (node, path) = (node.clone(), path.to_path_buf());
    fastctx_platform::task::spawn(async move {
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
                        crate::obligations::retry_job(&node, &path.with_extension("owed"), &error.to_string())
                    {
                        node.log.line(format_args!("recording the completion retry reason: {recording}"));
                    }
                }
            }
            let paused = {
                let path = path.clone();
                fastctx_platform::task::blocking(move || crate::completion_debt::pause(&path)).await
            };
            let pause = paused.ok().flatten().unwrap_or_else(|| again.next());
            tokio::select! {
                () = tokio::time::sleep(pause.min(fastctx_net::again::LONGEST)) => {}
                _ = ways.changed() => {}
            }
        }
        node.notifying_jobs.lock().expect("never poisoned").remove(&path);
    });
}

fn entries(directory: &Path) -> io::Result<Vec<std::path::PathBuf>> {
    match fastctx_platform::disk::read_dir(directory) {
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
    read_rows(directory, |rows| rows.get("pending-inputs", id.as_str()))
}
/// `id`'s input while it waits unsettled: no turn is known to have heard it,
/// and it is not among the uncertain.
pub(crate) fn unsettled(directory: &ConversationDir, id: &NoticeId) -> io::Result<Option<Pending>> {
    read_rows(directory, |rows| {
        if storage::delivered(rows, id.as_str())? || rows.get::<Pending>("uncertain-inputs", id.as_str())?.is_some() {
            return Ok(None);
        }
        rows.get("pending-inputs", id.as_str())
    })
}
