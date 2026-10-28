//! What is asked of the book. This computer's own interface sees the whole
//! of it; another machine asking for accounts sees what worked for them,
//! and what this computer's own user did when they are among them.

use std::sync::Arc;

use fastctx_storage::rusqlite::{self, params_from_iter, types::Value};
use fastctx_types::access::{Acting, Purpose, Written};
use fastctx_types::activity_book::{BookAnswer, BookQuery, Deed, Hour, Outcome, TouchHow, Touched};
use fastctx_types::ipc::Failure;
use fastctx_types::remote::{RemoteCall, RemoteReply};

use super::account_key;
use crate::Node;

/// The most deeds one answer carries.
const MOST: u32 = 2000;

/// The book of `machine`, or this computer's, as `acting` may see it.
pub(crate) async fn answer(
    node: &Arc<Node>,
    acting: &Acting,
    machine: Option<Written>,
    query: BookQuery,
) -> Result<BookAnswer, Failure> {
    let Some(target) = machine.filter(|machine| !node.is_here(machine)) else {
        return answer_here(node, None, query).await;
    };
    match node
        .call(acting, &target, Purpose::Hands, RemoteCall::Book { query })
        .await
        .map_err(|failure| Failure::new(failure.to_string()))??
    {
        RemoteReply::Book(answer) => Ok(*answer),
        _ => Err(Failure::new(format!("{target} answered something other than its activity"))),
    }
}

/// This computer's book; `acting` is another machine asking for its
/// accounts, none this computer's own interface.
pub(crate) async fn answer_here(
    node: &Arc<Node>,
    acting: Option<&Acting>,
    query: BookQuery,
) -> Result<BookAnswer, Failure> {
    let visible = acting.map(|acting| visible(node, acting));
    let machine = node.machine();
    let answer = node
        .database
        .read(move |connection| match query {
            BookQuery::Deeds { from, to, limit } => {
                let (deeds, more) =
                    deeds(connection, from.as_millisecond(), to.as_millisecond(), limit.min(MOST), visible.as_deref())?;
                Ok(BookAnswer::Deeds { machine, deeds, more })
            }
            BookQuery::Hours { from, to } => {
                let hours = hours(connection, from.as_millisecond(), to.as_millisecond(), visible.as_deref())?;
                Ok(BookAnswer::Hours { machine, hours })
            }
        })
        .await
        .map_err(|error| Failure::new(format!("reading the activity book: {error}")))?;
    Ok(answer)
}

/// The accounts whose deeds `acting` sees here: its own, and this
/// computer's own user's where one of them is logged in here.
fn visible(node: &Node, acting: &Acting) -> Vec<String> {
    let Acting::Accounts { accounts } = acting else { return vec![String::new()] };
    let mut keys: Vec<String> = accounts.iter().map(account_key).collect();
    if accounts.iter().any(|account| node.serves(&Acting::Here, &account.hub, &account.account)) {
        keys.push(String::new());
    }
    keys
}

fn filter(visible: Option<&[String]>, first: usize) -> String {
    match visible {
        None => String::new(),
        Some(keys) => {
            let marks: Vec<String> = (0..keys.len()).map(|index| format!("?{}", first + index)).collect();
            format!(" AND account IN ({})", if marks.is_empty() { "NULL".into() } else { marks.join(", ") })
        }
    }
}

fn bound(from: i64, to: i64, visible: Option<&[String]>) -> Vec<Value> {
    let mut values = vec![Value::Integer(from), Value::Integer(to)];
    values.extend(visible.into_iter().flatten().map(|key| Value::Text(key.clone())));
    values
}

fn deeds(
    connection: &rusqlite::Connection,
    from: i64,
    to: i64,
    limit: u32,
    visible: Option<&[String]>,
) -> fastctx_storage::Result<(Vec<Deed>, bool)> {
    let sql = format!(
        "SELECT job, began, ended, from_machine, session, agent, held, project, script, outcome, code, output,
                went, looked, copied
         FROM book_deed WHERE began >= ?1 AND began < ?2{} ORDER BY began DESC LIMIT {}",
        filter(visible, 3),
        limit + 1
    );
    let mut statement = connection.prepare(&sql)?;
    let mut deeds = statement
        .query_map(params_from_iter(bound(from, to, visible)), |row| {
            let at = |millisecond: i64| jiff::Timestamp::from_millisecond(millisecond).unwrap_or_default();
            // A row this release cannot read is left out, not the answer.
            let Ok(job) = row.get::<_, String>(0)?.parse() else { return Ok(None) };
            Ok(Some(Deed {
                job,
                began: at(row.get(1)?),
                ended: row.get::<_, Option<i64>>(2)?.map(at),
                from: row.get::<_, Option<String>>(3)?.and_then(|name| name.parse().ok()),
                session: row.get(4)?,
                agent: row.get::<_, Option<String>>(5)?.and_then(|agent| agent.parse().ok()),
                held: row.get(6)?,
                project: row.get::<_, Option<String>>(7)?.and_then(|town| town.parse().ok()),
                script: row.get(8)?,
                outcome: outcome(&row.get::<_, String>(9)?),
                code: row.get(10)?,
                output: row.get::<_, i64>(11)?.max(0) as u64,
                went: row.get::<_, String>(12)?.split_whitespace().filter_map(|name| name.parse().ok()).collect(),
                looked: row.get::<_, i64>(13)?.max(0) as u32,
                copied: row.get::<_, i64>(14)?.max(0) as u32,
                touched: Vec::new(),
            }))
        })?
        .filter_map(Result::transpose)
        .collect::<Result<Vec<_>, _>>()?;
    let more = deeds.len() > limit as usize;
    deeds.truncate(limit as usize);
    if !deeds.is_empty() {
        let marks: Vec<String> = (1..=deeds.len()).map(|index| format!("?{index}")).collect();
        let sql = format!(
            "SELECT job, path, how, added, removed, seen FROM book_touched WHERE job IN ({}) ORDER BY path",
            marks.join(", ")
        );
        let mut statement = connection.prepare(&sql)?;
        let jobs = deeds.iter().map(|deed| Value::Text(deed.job.to_string()));
        let rows = statement.query_map(params_from_iter(jobs), |row| {
            Ok((
                row.get::<_, String>(0)?,
                Touched {
                    path: row.get(1)?,
                    how: how(&row.get::<_, String>(2)?),
                    added: row.get::<_, Option<i64>>(3)?.map(|n| n.max(0) as u32),
                    removed: row.get::<_, Option<i64>>(4)?.map(|n| n.max(0) as u32),
                    seen: row.get(5)?,
                },
            ))
        })?;
        let mut by_job: std::collections::HashMap<String, Vec<Touched>> = std::collections::HashMap::new();
        for row in rows {
            let (job, touched) = row?;
            by_job.entry(job).or_default().push(touched);
        }
        for deed in &mut deeds {
            deed.touched = by_job.remove(deed.job.as_str()).unwrap_or_default();
        }
    }
    Ok((deeds, more))
}

fn hours(
    connection: &rusqlite::Connection,
    from: i64,
    to: i64,
    visible: Option<&[String]>,
) -> fastctx_storage::Result<Vec<Hour>> {
    let accounts = filter(visible, 3);
    let sql = format!(
        "SELECT (began / 3600000) * 3600000, COALESCE(session, ''), COALESCE(agent, ''), held,
                COUNT(*),
                SUM(CASE WHEN outcome IN ('failed', 'lost') THEN 1 ELSE 0 END),
                SUM(MAX(COALESCE(ended, began) - began, 0)),
                SUM((SELECT COUNT(*) FROM book_touched WHERE book_touched.job = book_deed.job))
         FROM book_deed WHERE began >= ?1 AND began < ?2{accounts}
         GROUP BY 1, 2, 3, 4
         UNION ALL
         SELECT at, session, agent, held, SUM(runs), SUM(failed), SUM(busy), SUM(files)
         FROM book_hour WHERE at > ?1 - 3600000 AND at < ?2{accounts}
         GROUP BY 1, 2, 3, 4
         ORDER BY 1"
    );
    let mut statement = connection.prepare(&sql)?;
    let hours = statement
        .query_map(params_from_iter(bound(from, to, visible)), |row| {
            let session: String = row.get(1)?;
            let agent: String = row.get(2)?;
            Ok(Hour {
                at: jiff::Timestamp::from_millisecond(row.get(0)?).unwrap_or_default(),
                session: (!session.is_empty()).then_some(session),
                agent: agent.parse().ok(),
                held: row.get(3)?,
                runs: row.get::<_, i64>(4)?.max(0) as u32,
                failed: row.get::<_, i64>(5)?.max(0) as u32,
                busy: row.get::<_, i64>(6)?.max(0) as u64,
                files: row.get::<_, i64>(7)?.max(0) as u32,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(hours)
}

fn outcome(word: &str) -> Outcome {
    match word {
        "running" => Outcome::Running,
        "done" => Outcome::Done,
        "stopped" => Outcome::Stopped,
        "lost" => Outcome::Lost,
        _ => Outcome::Failed,
    }
}

fn how(word: &str) -> TouchHow {
    match word {
        "added" => TouchHow::Added,
        "removed" => TouchHow::Removed,
        "copied" => TouchHow::Copied,
        _ => TouchHow::Changed,
    }
}
