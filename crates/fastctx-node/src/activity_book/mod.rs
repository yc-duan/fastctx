//! This machine's activity book (`world-layers.md`, 活动账本): one deed for
//! each script `run` ran here, written as its job changes, with the files
//! it touched. Deeds are kept 90 days, then folded into counts by the hour,
//! kept a year. It lives in the node's database, beside the composer's.

mod query;
mod seen;

pub(crate) use query::{answer, answer_here};

use std::sync::Arc;

use fastctx_storage::rusqlite::{self, params};
use fastctx_types::access::Acting;
use fastctx_types::activity_book::{Outcome, TouchHow};
use fastctx_types::run::{EndedHow, ExitStatus, JobOwner, JobRecord};
use fastctx_types::run_facts::{StepDid, WriteFact};
use fastctx_types::write::ChangeKind;

use crate::Node;

/// The book's tables; idempotent, installed with the node's database.
pub(crate) const SCHEMA: &str = "
DROP TABLE IF EXISTS book_touched;
DROP TABLE IF EXISTS book_deed;
CREATE TABLE IF NOT EXISTS book_deeds (
    deed TEXT PRIMARY KEY,
    job TEXT NOT NULL,
    began INTEGER NOT NULL,
    ended INTEGER,
    from_machine TEXT,
    session TEXT,
    agent TEXT,
    held INTEGER NOT NULL DEFAULT 0,
    project TEXT,
    account TEXT NOT NULL DEFAULT '',
    script TEXT NOT NULL,
    outcome TEXT NOT NULL,
    code INTEGER,
    output INTEGER NOT NULL DEFAULT 0,
    went TEXT NOT NULL DEFAULT '',
    looked INTEGER NOT NULL DEFAULT 0,
    copied INTEGER NOT NULL DEFAULT 0
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS book_deeds_began ON book_deeds(began);
CREATE TABLE IF NOT EXISTS book_touches (
    deed TEXT NOT NULL,
    path TEXT NOT NULL,
    how TEXT NOT NULL,
    added INTEGER,
    removed INTEGER,
    seen INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (deed, path)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS book_hour (
    at INTEGER NOT NULL,
    session TEXT NOT NULL DEFAULT '',
    agent TEXT NOT NULL DEFAULT '',
    held INTEGER NOT NULL DEFAULT 0,
    account TEXT NOT NULL DEFAULT '',
    runs INTEGER NOT NULL,
    failed INTEGER NOT NULL,
    busy INTEGER NOT NULL,
    files INTEGER NOT NULL,
    PRIMARY KEY (at, session, agent, held, account)
) WITHOUT ROWID;
";

/// How long deeds are kept whole, and how long their hourly counts.
const DEEDS_KEPT: jiff::SignedDuration = jiff::SignedDuration::from_hours(90 * 24);
const HOURS_KEPT: jiff::SignedDuration = jiff::SignedDuration::from_hours(366 * 24);

/// The longest script kept; the rest is the job's own record while it lasts.
const SCRIPT: usize = 2000;

/// One deed as a job's record stands now.
struct Row {
    deed: String,
    job: String,
    began: i64,
    ended: Option<i64>,
    from: Option<String>,
    session: Option<String>,
    agent: Option<String>,
    held: bool,
    project: Option<String>,
    account: String,
    script: String,
    outcome: Outcome,
    code: Option<i32>,
    output: i64,
    went: String,
    looked: i64,
    copied: i64,
    touched: Vec<Touch>,
}

struct Touch {
    path: String,
    how: TouchHow,
    added: Option<u32>,
    removed: Option<u32>,
}

/// Writes the deed of the job `record` as it now stands, with what its
/// steps and writes say it did. Called on every change of a job; it never
/// holds the caller up, and a failure is only logged.
pub(crate) fn note(
    node: &Arc<Node>,
    record: &JobRecord,
    writes: &[WriteFact],
    steps: &[fastctx_types::run_facts::RunStep],
) {
    if record.is_sentinel() {
        return;
    }
    let row = row(node, record, writes, steps);
    let ended = row.ended.is_some();
    let size = row.script.len() + row.touched.iter().map(|touch| touch.path.len() + 32).sum::<usize>() + 512;
    let database = node.database.clone();
    let log = node.log.clone();
    tokio::spawn(async move {
        if let Err(error) = database.write(size, move |connection| keep(connection, &row)).await {
            log.line(format_args!("keeping the activity book: {error}"));
        }
    });
    if ended {
        seen::look(node, record);
    }
}

fn row(node: &Node, record: &JobRecord, writes: &[WriteFact], steps: &[fastctx_types::run_facts::RunStep]) -> Row {
    // A hop sent here works for the account that sent it, whose session has
    // no record on this machine.
    let sent_for = record.on.as_ref().and_then(|on| on.account.as_ref()).or(record.account.as_ref());
    let (session, from, agent, held, account) = match &record.owner {
        JobOwner::Resident(address) => {
            let here = node.is_me(&address.machine);
            let about = here.then(|| node.residents.about(&address.resident).map(|(about, _)| about)).flatten();
            let account = match (sent_for, about.as_ref().map(|about| &about.acting)) {
                (Some(account), _) => account_key(account),
                (None, Some(Acting::Accounts { accounts })) => accounts.first().map(account_key).unwrap_or_default(),
                _ => String::new(),
            };
            (
                Some(address.tag()),
                (!here).then(|| address.machine.to_string()),
                about.as_ref().map(|about| about.host.agent.to_string()),
                about.as_ref().is_some_and(|about| about.world_managed()),
                account,
            )
        }
        JobOwner::Page { .. } => (None, None, None, true, sent_for.map(account_key).unwrap_or_default()),
    };
    let outcome = match &record.end {
        None => Outcome::Running,
        Some(end) => match &end.how {
            EndedHow::Stopped => Outcome::Stopped,
            EndedHow::Interrupted { .. } => Outcome::Lost,
            EndedHow::Failed { .. } => Outcome::Failed,
            EndedHow::Finished => match record.exit.as_ref().map(|exit| &exit.status) {
                Some(ExitStatus::Code(0)) => Outcome::Done,
                _ => Outcome::Failed,
            },
        },
    };
    let code = record.exit.as_ref().and_then(|exit| match exit.status {
        ExitStatus::Code(code) => i32::try_from(code).ok().or(Some(code as i32)),
        ExitStatus::Signal(_) => None,
    });
    let output = [Some(&record.output), record.stderr.as_ref()]
        .into_iter()
        .flatten()
        .filter_map(|path| std::fs::metadata(path).ok())
        .map(|meta| meta.len() as i64)
        .sum();
    let mut went = Vec::new();
    let (mut looked, mut copied) = (0i64, 0i64);
    let mut touched = Vec::new();
    for step in steps {
        match &step.did {
            StepDid::On { to, .. } => {
                let to = to.to_string();
                if !went.contains(&to) {
                    went.push(to);
                }
            }
            StepDid::View { paths, .. } => looked += paths.len() as i64,
            StepDid::Copy { to, files, .. } => {
                copied += *files as i64;
                if node.is_me(&to.machine) {
                    touched.push(Touch { path: to.path.clone(), how: TouchHow::Copied, added: None, removed: None });
                }
            }
            _ => {}
        }
    }
    for write in writes.iter().filter(|write| node.is_me(&write.machine)) {
        let how = match write.kind {
            ChangeKind::Added => TouchHow::Added,
            ChangeKind::Modified => TouchHow::Changed,
            ChangeKind::Deleted => TouchHow::Removed,
        };
        touched.push(Touch { path: write.path.clone(), how, added: write.added, removed: write.removed });
    }
    let mut script = record.script.clone();
    if script.len() > SCRIPT {
        let mut end = SCRIPT;
        while !script.is_char_boundary(end) {
            end -= 1;
        }
        script.truncate(end);
        script.push('…');
    }
    Row {
        deed: deed(record),
        job: record.id.to_string(),
        began: record.started_at.as_millisecond(),
        ended: record.end.as_ref().map(|end| end.at.as_millisecond()),
        from,
        session,
        agent,
        held,
        project: record.town.as_ref().map(ToString::to_string),
        account,
        script,
        outcome,
        code,
        output,
        went: went.join(" "),
        looked,
        copied,
        touched,
    }
}

/// Which deed a job is. A job's short name is its conversation's only, and
/// comes again once its files are gone: the deed is the job of that owner
/// begun at that time.
pub(super) fn deed(record: &JobRecord) -> String {
    format!("{} {} {}", record.owner, record.id, record.started_at.as_millisecond())
}

/// How a deed's account is kept: empty for this computer's own user.
pub(super) fn account_key(account: &fastctx_types::access::HubAccount) -> String {
    format!("{}/{}", account.hub, account.account)
}

pub(super) fn outcome_word(outcome: Outcome) -> &'static str {
    match outcome {
        Outcome::Running => "running",
        Outcome::Done => "done",
        Outcome::Failed => "failed",
        Outcome::Stopped => "stopped",
        Outcome::Lost => "lost",
    }
}

pub(super) fn how_word(how: TouchHow) -> &'static str {
    match how {
        TouchHow::Added => "added",
        TouchHow::Changed => "changed",
        TouchHow::Removed => "removed",
        TouchHow::Copied => "copied",
    }
}

/// Changes of one job may be told in any order: what is known of its end
/// is never taken back by an earlier telling.
fn keep(connection: &rusqlite::Connection, row: &Row) -> fastctx_storage::Result<()> {
    connection.execute(
        "INSERT INTO book_deeds (deed, job, began, ended, from_machine, session, agent, held, project, account,
             script, outcome, code, output, went, looked, copied)
         VALUES (?17, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
         ON CONFLICT(deed) DO UPDATE SET
             ended = COALESCE(excluded.ended, book_deeds.ended),
             outcome = CASE WHEN excluded.outcome = 'running' THEN book_deeds.outcome ELSE excluded.outcome END,
             code = COALESCE(excluded.code, book_deeds.code),
             output = MAX(excluded.output, book_deeds.output),
             went = CASE WHEN length(excluded.went) >= length(book_deeds.went) THEN excluded.went ELSE book_deeds.went END,
             looked = MAX(excluded.looked, book_deeds.looked),
             copied = MAX(excluded.copied, book_deeds.copied)",
        params![
            row.job,
            row.began,
            row.ended,
            row.from,
            row.session,
            row.agent,
            row.held,
            row.project,
            row.account,
            row.script,
            outcome_word(row.outcome),
            row.code,
            row.output,
            row.went,
            row.looked,
            row.copied,
            row.deed,
        ],
    )?;
    for touch in &row.touched {
        connection.execute(
            "INSERT OR REPLACE INTO book_touches (deed, path, how, added, removed, seen) VALUES (?1, ?2, ?3, ?4, ?5, 0)",
            params![row.deed, touch.path, how_word(touch.how), touch.added, touch.removed],
        )?;
    }
    Ok(())
}

/// Folds deeds older than 90 days into their hours, and lets hours older
/// than a year go. Part of the node's hourly maintenance.
pub(crate) fn fold(node: &Node) -> std::io::Result<()> {
    let now = jiff::Timestamp::now();
    let deeds_before = now.checked_sub(DEEDS_KEPT).map_err(std::io::Error::other)?.as_millisecond();
    let hours_before = now.checked_sub(HOURS_KEPT).map_err(std::io::Error::other)?.as_millisecond();
    node.database
        .write_now(512, move |connection| {
            connection.execute(
                "INSERT INTO book_hour (at, session, agent, held, account, runs, failed, busy, files)
                 SELECT (began / 3600000) * 3600000, COALESCE(session, ''), COALESCE(agent, ''), held, account,
                        COUNT(*),
                        SUM(CASE WHEN outcome IN ('failed', 'lost') THEN 1 ELSE 0 END),
                        SUM(MAX(COALESCE(ended, began) - began, 0)),
                        SUM((SELECT COUNT(*) FROM book_touches WHERE book_touches.deed = book_deeds.deed))
                 FROM book_deeds WHERE began < ?1 AND ended IS NOT NULL
                 GROUP BY 1, 2, 3, 4, 5
                 ON CONFLICT(at, session, agent, held, account) DO UPDATE SET
                     runs = book_hour.runs + excluded.runs,
                     failed = book_hour.failed + excluded.failed,
                     busy = book_hour.busy + excluded.busy,
                     files = book_hour.files + excluded.files",
                params![deeds_before],
            )?;
            connection.execute(
                "DELETE FROM book_touches WHERE deed IN (SELECT deed FROM book_deeds WHERE began < ?1 AND ended IS NOT NULL)",
                params![deeds_before],
            )?;
            connection
                .execute("DELETE FROM book_deeds WHERE began < ?1 AND ended IS NOT NULL", params![deeds_before])?;
            connection.execute("DELETE FROM book_hour WHERE at < ?1", params![hours_before])?;
            Ok(())
        })
        .map_err(std::io::Error::other)
}
