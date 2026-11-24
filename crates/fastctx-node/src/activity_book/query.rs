//! What is asked of the book. This computer's own interface sees the whole
//! of it; another machine asking for accounts sees what worked for them,
//! and what this computer's own user did when they are among them.

use std::collections::BTreeSet;
use std::sync::Arc;

use fastctx_storage::rusqlite::{self, params_from_iter, types::Value};
use fastctx_types::access::{Acting, Purpose, Written};
use fastctx_types::activity_book::{BookAnswer, BookQuery, Deed, Hour, Outcome};
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
    // The machine, which tells all of it; from the copies here (plan 3-5)
    // where it is away or does not answer, as a login's use is
    // (`logins::remote::usage`): no machine writes them since beta.26.
    let copied = || {
        let (asking, whose, written, asked) = (node.clone(), acting.clone(), target.clone(), query.clone());
        fastctx_platform::task::spawn_blocking(move || super::copied::answer(&asking, &whose, &written, &asked))
    };
    if crate::timeline::copy::away(node, &target.name)
        && let Ok(Some(answer)) = copied().await
    {
        return Ok(answer);
    }
    let asked = match node.call(acting, &target, Purpose::Hands, RemoteCall::Book { query: query.clone() }).await {
        Ok(Ok(RemoteReply::Book(answer))) => return Ok(*answer),
        Ok(Ok(_)) => return Err(Failure::new(format!("{target} answered something other than its activity"))),
        Ok(Err(failure)) => failure,
        Err(failure) => crate::authorization::call_failure(failure),
    };
    match copied().await {
        Ok(Some(answer)) => Ok(answer),
        _ => Err(asked),
    }
}

/// This computer's book; `acting` is another machine asking for its
/// accounts, none this computer's own interface.
pub(crate) async fn answer_here(
    node: &Arc<Node>,
    acting: Option<&Acting>,
    query: BookQuery,
) -> Result<BookAnswer, Failure> {
    let preparing = node.clone();
    fastctx_platform::task::spawn_blocking(move || super::migrate::ensure(&preparing))
        .await
        .map_err(|e| Failure::new(e.to_string()))?
        .map_err(crate::record_migration::failure)?;
    let visible = acting.and_then(|acting| visible(node, acting));
    // Seen by one that works here, every account's deeds: whose each is
    // stays said, but another account's names no conversation — one of its
    // own elsewhere, or one it was assisted in, is not this machine's to
    // tell (`net-shared-machine-visibility` 2026-11-10).
    let unnamed = acting.filter(|_| visible.is_none()).map(own_keys);
    let machine = node.machine();
    let answer = node
        .book_database
        .read(move |connection| match query {
            BookQuery::Deeds { from, to, limit } => {
                let (from, to) = (from.as_millisecond(), to.as_millisecond());
                let (deeds, more) = deeds(connection, from, to, limit.min(MOST), visible.as_deref(), unnamed.as_ref())?;
                Ok(BookAnswer::Deeds { machine, deeds, more })
            }
            BookQuery::Hours { from, to } => {
                let hours = hours(connection, from.as_millisecond(), to.as_millisecond(), visible.as_deref(), unnamed.as_ref())?;
                Ok(BookAnswer::Hours { machine, hours })
            }
        })
        .await
        .map_err(|error| crate::record_migration::storage_failure("reading the activity book", error))?;
    Ok(answer)
}

/// The accounts whose deeds `acting` sees here, none where it sees every
/// account's: one that may work on this machine does, each deed with its
/// account (`net-shared-machine-visibility` 2026-11-10: 能在这台机器上干活的
/// 账号都看得见，并标明是哪个账号的); another its own, and this computer's
/// own user's where one of them is logged in here.
fn visible(node: &Node, acting: &Acting) -> Option<Vec<String>> {
    let Acting::Accounts { accounts } = acting else { return Some(vec![String::new()]) };
    let works = accounts.iter().any(|account| {
        node.link_of(&account.hub).is_some_and(|link| {
            let view = link.view();
            view.rights(&account.account, &view.me).hands
        })
    });
    if works {
        return None;
    }
    let mut keys: Vec<String> = accounts.iter().map(account_key).collect();
    if accounts.iter().any(|account| node.serves(&Acting::Here, &account.hub, &account.account)) {
        keys.push(String::new());
    }
    Some(keys)
}

/// The keys of the accounts `acting` names.
fn own_keys(acting: &Acting) -> BTreeSet<String> {
    match acting {
        Acting::Accounts { accounts } => accounts.iter().map(account_key).collect(),
        Acting::Here => Default::default(),
    }
}

/// Whether a row of the account `key` names its conversation to one seeing
/// as the accounts `own`: its own, and this machine's own user's.
fn named(own: Option<&BTreeSet<String>>, key: &str) -> bool {
    own.is_none_or(|own| key.is_empty() || own.contains(key))
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
    unnamed: Option<&BTreeSet<String>>,
) -> fastctx_storage::Result<(Vec<Deed>, bool)> {
    let sql = format!(
        "SELECT job, began, ended, from_machine, session, agent, held, project, script, outcome, code, output,
                went, looked, copied, deed, account
         FROM book_deeds WHERE NOT EXISTS (SELECT 1 FROM book_folding f WHERE f.deed=book_deeds.deed AND f.applied=1) AND began >= ?1 AND began < ?2{} ORDER BY began DESC LIMIT {}",
        filter(visible, 3),
        limit + 1
    );
    let mut statement = connection.prepare(&sql)?;
    let mut deeds = statement
        .query_map(params_from_iter(bound(from, to, visible)), |row| {
            let at = |millisecond: i64| jiff::Timestamp::from_millisecond(millisecond).unwrap_or_default();
            // A row this release cannot read is left out, not the answer.
            let Ok(job) = row.get::<_, String>(0)?.parse() else { return Ok(None) };
            let key: String = row.get(15)?;
            let account: String = row.get(16)?;
            let mut session: Option<String> = row.get(4)?;
            if !named(unnamed, &account) {
                session = None;
            }
            Ok(Some((
                key,
                Deed {
                    job,
                    began: at(row.get(1)?),
                    ended: row.get::<_, Option<i64>>(2)?.map(at),
                    from: row.get::<_, Option<String>>(3)?.and_then(|name| name.parse().ok()),
                    session,
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
                    account: account.split_once('/').and_then(|(_, name)| name.parse().ok()),
                },
            )))
        })?
        .filter_map(Result::transpose)
        .collect::<Result<Vec<_>, _>>()?;
    let more = deeds.len() > limit as usize;
    deeds.truncate(limit as usize);
    super::query_details::load(connection, &mut deeds)?;
    Ok((deeds.into_iter().map(|(_, deed)| deed).collect(), more))
}

fn hours(
    connection: &rusqlite::Connection,
    from: i64,
    to: i64,
    visible: Option<&[String]>,
    unnamed: Option<&BTreeSet<String>>,
) -> fastctx_storage::Result<Vec<Hour>> {
    let accounts = filter(visible, 3);
    let sql = format!(
        "SELECT (began / 3600000) * 3600000, COALESCE(session, ''), COALESCE(agent, ''), held,
                COUNT(*),
                SUM(CASE WHEN outcome IN ('failed', 'lost') THEN 1 ELSE 0 END),
                SUM(MAX(COALESCE(ended, began) - began, 0)),
                SUM((SELECT COUNT(*) FROM book_touches WHERE book_touches.deed = book_deeds.deed)),
                account
         FROM book_deeds WHERE NOT EXISTS (SELECT 1 FROM book_folding f WHERE f.deed=book_deeds.deed AND f.applied=1) AND began >= ?1 AND began < ?2{accounts}
         GROUP BY 1, 2, 3, 4, 9
         UNION ALL
         SELECT at, session, agent, held, SUM(runs), SUM(failed), SUM(busy), SUM(files), account
         FROM book_hour WHERE at > ?1 - 3600000 AND at < ?2{accounts}
         GROUP BY 1, 2, 3, 4, 9
         ORDER BY 1"
    );
    let mut statement = connection.prepare(&sql)?;
    let rows = statement
        .query_map(params_from_iter(bound(from, to, visible)), |row| {
            let session: String = row.get(1)?;
            let agent: String = row.get(2)?;
            let account: String = row.get::<_, Option<String>>(8)?.unwrap_or_default();
            Ok(Hour {
                at: jiff::Timestamp::from_millisecond(row.get(0)?).unwrap_or_default(),
                session: (!session.is_empty() && named(unnamed, &account)).then_some(session),
                agent: agent.parse().ok(),
                held: row.get(3)?,
                runs: row.get::<_, i64>(4)?.max(0) as u32,
                failed: row.get::<_, i64>(5)?.max(0) as u32,
                busy: row.get::<_, i64>(6)?.max(0) as u64,
                files: row.get::<_, i64>(7)?.max(0) as u32,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    // Kept apart by account to say whose names a conversation; told as the
    // book holds them, one hour of one session alike, however many accounts.
    // Found again by what they are told by, not by looking through all told
    // so far: a year of hours is read at once.
    let mut hours: Vec<Hour> = Vec::with_capacity(rows.len());
    let mut at: std::collections::HashMap<String, usize> = std::collections::HashMap::with_capacity(rows.len());
    for hour in rows {
        let key = format!("{:?}\u{0}{:?}\u{0}{:?}\u{0}{:?}", hour.at, hour.session, hour.agent, hour.held);
        match at.get(&key) {
            Some(&index) => {
                let kept = &mut hours[index];
                kept.runs += hour.runs;
                kept.failed += hour.failed;
                kept.busy += hour.busy;
                kept.files += hour.files;
            }
            None => {
                at.insert(key, hours.len());
                hours.push(hour);
            }
        }
    }
    Ok(hours)
}

pub(super) fn outcome(word: &str) -> Outcome {
    match word {
        "running" => Outcome::Running,
        "done" => Outcome::Done,
        "stopped" => Outcome::Stopped,
        "lost" => Outcome::Lost,
        _ => Outcome::Failed,
    }
}
