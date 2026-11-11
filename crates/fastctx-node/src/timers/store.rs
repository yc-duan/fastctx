//! SQL owns timer generations and their due index. Only active claims and a
//! disk-failure backoff live in memory; no registry lock covers storage work.
//! A timer stays until it fires or is cancelled, or its conversation is
//! deleted; a sentinel's until the sentinel acknowledges it.

use crate::records::{self, Source};
use fastctx_storage::{Priority, Result};
use fastctx_types::{
    NoticeId,
    layout::ConversationDir,
    timers::{Action, Owner, Timer},
};
use jiff::Timestamp;
use sha2::{Digest, Sha256};
use std::{
    io,
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::Notify;

const FAMILY: &str = "timers";
use super::claims::Claims;

pub struct Store {
    pub(super) directory: ConversationDir,
    pub(super) claims: Mutex<Claims>,
    pub changed: Notify,
}
impl Store {
    pub fn open(path: &Path) -> io::Result<Self> {
        let (directory, _, _, _) = crate::record_database::item(&path.join(".timers"))?;
        let folder = path.to_path_buf();
        records::ensure_each(&directory, FAMILY, move |push| {
            let entries = match fastctx_platform::disk::read_dir(&folder) {
                Ok(entries) => entries,
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
                Err(error) => return Err(error),
            };
            for entry in entries {
                let source = entry?.path();
                if source.extension().is_none_or(|ext| ext != "json") {
                    continue;
                }
                crate::record_migration::file(&source, push, |push| {
                    let bytes = fastctx_platform::disk::read(&source)?;
                    let timer: Timer = serde_json::from_slice(&bytes)?;
                    if source != folder.join(format!("{:x}.json", Sha256::digest(timer.id.as_bytes()))) {
                        return Err(crate::record_migration::errors::invalid("timer id does not match its file"));
                    }
                    push(
                        Source::new(timer.id.clone(), source.clone(), bytes)?
                            .scoped(scope(&timer.owner).to_string())
                            .ordered(timer.next_attempt)
                            .categorized(serde_json::to_string(&timer.owner)?),
                    )
                })?;
            }
            Ok(())
        })?;
        Ok(Self { directory, claims: Mutex::new(Claims::default()), changed: Notify::new() })
    }
    pub(super) async fn work<T: Send + 'static>(
        self: &Arc<Self>,
        action: impl FnOnce(&Self) -> io::Result<T> + Send + 'static,
    ) -> io::Result<T> {
        let store = self.clone();
        fastctx_platform::task::spawn_blocking(move || action(&store)).await.map_err(io::Error::other)?
    }
    pub async fn set(
        self: &Arc<Self>,
        id: String,
        owner: Owner,
        due: Timestamp,
        action: Action,
        delivery: NoticeId,
    ) -> io::Result<Timer> {
        if id.is_empty() {
            return Err(io::Error::other("a timer needs an id"));
        }
        let timer = Timer {
            id,
            created_at: Some(Timestamp::now()),
            owner,
            due,
            action,
            delivery,
            attempts: 0,
            next_attempt: due,
            fired_at: None,
            error: None,
        };
        self.work(move |store| {
            let result = records::write_at(
                &store.directory,
                Priority::Interactive,
                serde_json::to_vec(&timer)?.len(),
                move |rows| {
                    if super::headers::get(rows, &timer.id, false)?
                        .is_some_and(|old| old.owner.as_ref() != Some(&timer.owner))
                    {
                        return Err(fastctx_storage::Error::new("cancel a timer before changing its owner"));
                    }
                    save(rows, &timer)?;
                    Ok(timer)
                },
            )?;
            {
                let mut claims = store.claims.lock().expect("timer claims");
                claims.settled(&result.id);
                claims.replaced(&result.id, &result.delivery);
            }
            store.changed.notify_one();
            Ok(result)
        })
        .await
    }
    pub async fn get(self: &Arc<Self>, id: &str) -> io::Result<Option<Timer>> {
        let id = id.to_string();
        self.work(move |store| records::read(&store.directory, |rows| get_timer_bounded(rows, &id))).await
    }
    pub(super) async fn page(
        self: &Arc<Self>,
        owner: Option<&Owner>,
        after: Option<&str>,
    ) -> io::Result<super::page::Page> {
        let owner = owner.cloned();
        let after = after.unwrap_or_default().to_string();
        self.work(move |store| records::read(&store.directory, |rows| super::page::read(rows, owner.as_ref(), &after)))
            .await
    }
    pub async fn list(self: &Arc<Self>, owner: Option<&Owner>) -> io::Result<Vec<Timer>> {
        let owner = owner.cloned();
        self.work(move |store| store.list_now(owner.as_ref())).await
    }
    fn list_now(&self, owner: Option<&Owner>) -> io::Result<Vec<Timer>> {
        let category = owner.map(serde_json::to_string).transpose()?;
        records::read(&self.directory, |rows| {
            let mut query=rows.connection().prepare_cached("SELECT body FROM conversation_records r WHERE family='timers' AND NOT EXISTS(SELECT 1 FROM node_conversation_transfers t WHERE t.name=r.conversation AND t.rejected=0) AND (?1 IS NULL OR category=?1) ORDER BY identity")?;
            query.query_map([category], |row| row.get::<_, Vec<u8>>(0))?.map(|row| records::decode(&row?)).collect()
        })
    }
    fn candidate(&self) -> io::Result<Option<(Timestamp, Timer)>> {
        let claims = self.claims.lock().expect("timer claims").clone();
        // Only entries excluded by the small live claim/backoff set need to be
        // skipped. The next unexcluded row bounds every later candidate.
        let limit = claims.active.len() + claims.delayed.len() + 1;
        let rows = records::read(&self.directory, |rows| {
            let mut query = rows.connection().prepare_cached(
                "SELECT body FROM conversation_records r WHERE family='timers' AND NOT EXISTS(SELECT 1 FROM node_conversation_transfers t WHERE t.name=r.conversation AND t.rejected=0) ORDER BY arrival,identity LIMIT ?1",
            )?;
            query
                .query_map([limit as i64], |row| row.get::<_, Vec<u8>>(0))?
                .map(|row| records::decode::<Timer>(&row?))
                .collect::<Result<Vec<_>>>()
        })?;
        Ok(rows
            .into_iter()
            .filter(|row| !claims.active.contains_key(row.id.as_str()))
            .map(|row| {
                let due = claims
                    .delayed
                    .get(row.id.as_str())
                    .and_then(|generations| generations.get(&row.delivery))
                    .copied()
                    .unwrap_or(row.next_attempt)
                    .max(row.next_attempt);
                (due, row)
            })
            .min_by(|a, b| (&a.0, &a.1.id).cmp(&(&b.0, &b.1.id))))
    }
    pub async fn next(self: &Arc<Self>) -> io::Result<Option<Timestamp>> {
        self.work(|store| Ok(store.candidate()?.map(|(at, _)| at))).await
    }
    pub async fn claim(self: &Arc<Self>, now: Timestamp) -> io::Result<Option<Timer>> {
        self.work(move |store| {
            loop {
                let Some((at, timer)) = store.candidate()? else { return Ok(None) };
                if at > now {
                    return Ok(None);
                }
                let mut claims = store.claims.lock().expect("timer claims");
                claims.delayed.retain(|_, generations| {
                    generations.retain(|_, until| *until > now);
                    !generations.is_empty()
                });
                if claims.active.contains_key(timer.id.as_str()) {
                    continue;
                }
                let key = claims.alias(&timer.id, &timer.delivery);
                claims.active.insert(key, timer.delivery.clone());
                claims.delayed.remove(timer.id.as_str());
                return Ok(Some(timer));
            }
        })
        .await
    }
    pub fn unclaim(&self, timer: &Timer) {
        self.claims.lock().expect("timer claims").unclaim(timer);
        self.changed.notify_one();
    }
    async fn update(
        self: &Arc<Self>,
        timer: &Timer,
        action: impl FnOnce(&mut Timer) -> Result<()> + Send + 'static,
    ) -> io::Result<Option<Timer>> {
        let (id, delivery) = (timer.id.clone(), timer.delivery.clone());
        self.work(move |store| {
            let result = records::write_at(&store.directory, Priority::Interactive, 512, move |rows| {
                let Some(mut timer) = get_timer(rows, &id)?.filter(|row| row.delivery == delivery) else {
                    return Ok(None);
                };
                action(&mut timer)?;
                save(rows, &timer)?;
                Ok(Some(timer))
            })?;
            store.changed.notify_one();
            Ok(result)
        })
        .await
    }
    pub async fn begin(self: &Arc<Self>, timer: &Timer, now: Timestamp) -> io::Result<Option<Timer>> {
        self.update(timer, move |row| {
            row.fired_at.get_or_insert(now);
            row.attempts = row.attempts.saturating_add(1);
            Ok(())
        })
        .await
    }
    pub async fn retry(self: &Arc<Self>, timer: &Timer, now: Timestamp, error: String) -> io::Result<()> {
        self.update(timer, move |row| {
            row.next_attempt = now
                .checked_add(fastctx_net::again::backoff(row.attempts.max(1)))
                .map_err(|e| fastctx_storage::Error::new(e.to_string()))?;
            row.error = Some(error);
            Ok(())
        })
        .await
        .map(|_| ())
    }
    pub async fn hold(self: &Arc<Self>, timer: &Timer, until: Timestamp) -> io::Result<()> {
        self.update(timer, move |row| {
            row.next_attempt = until;
            row.error = None;
            Ok(())
        })
        .await
        .map(|_| ())
    }
    /// Failed persistence changes only scheduling, never the durable first firing.
    pub fn release(&self, timer: &Timer, now: Timestamp) {
        {
            let mut claims = self.claims.lock().expect("timer claims");
            let key = claims.alias(&timer.id, &timer.delivery);
            claims
                .delayed
                .entry(key)
                .or_default()
                .insert(timer.delivery.clone(), now.checked_add(Duration::from_secs(60)).unwrap_or(now));
        }
        self.unclaim(timer);
    }
    pub async fn complete(self: &Arc<Self>, timer: &Timer) -> io::Result<bool> {
        let (id, delivery, timer_id) = (timer.id.clone(), timer.delivery.clone(), timer.id.clone());
        self.work(move |store| {
            let removed = records::write_at(&store.directory, Priority::Interactive, 128, move |rows| {
                // Its header says whether it is still this firing's: a row
                // an older node left with a very large payload completes
                // without that payload being read.
                if super::headers::delivery(rows, &id)?.is_none_or(|current| current != delivery.as_str()) {
                    return Ok(false);
                }
                delete_timer(rows, &id)
            })?;
            if removed {
                store.claims.lock().expect("timer claims").settled(&timer_id);
            }
            store.changed.notify_one();
            Ok(removed)
        })
        .await
    }
    pub async fn cancel(self: &Arc<Self>, id: &str) -> io::Result<bool> {
        let id = id.to_string();
        self.work(move |store| {
            let cancelled = id.clone();
            let removed =
                records::write_at(&store.directory, Priority::Control, 128, move |rows| delete_timer(rows, &id))?;
            store.claims.lock().expect("timer claims").settled(&cancelled);
            store.changed.notify_one();
            Ok(removed)
        })
        .await
    }
}
fn save(rows: &records::Rows<'_>, timer: &Timer) -> Result<()> {
    let rows = rows.scoped(scope(&timer.owner));
    rows.put_ordered(FAMILY, &timer.id, timer, timer.next_attempt)?;
    rows.connection().execute(
        "UPDATE conversation_records SET category=?3 WHERE conversation=?1 AND family='timers' AND identity=?2",
        (
            rows.conversation(),
            &timer.id,
            serde_json::to_string(&timer.owner).map_err(|e| fastctx_storage::Error::new(e.to_string()))?,
        ),
    )?;
    Ok(())
}

fn scope(owner: &Owner) -> &str {
    match owner {
        Owner::Conversation { resident } | Owner::Goal { resident } => resident.as_str(),
        _ => "@node",
    }
}
/// The whole timer, whatever its size: what fires it and reschedules it
/// reads the row the worker already chose, also one an older node left with
/// a payload past every bound of today's.
fn get_timer(rows: &records::Rows<'_>, id: &str) -> Result<Option<Timer>> {
    use fastctx_storage::rusqlite::OptionalExtension;
    let bytes: Option<Vec<u8>> = rows
        .connection()
        .query_row("SELECT body FROM conversation_records r WHERE family='timers' AND identity=?1 AND NOT EXISTS(SELECT 1 FROM node_conversation_transfers t WHERE t.name=r.conversation AND t.rejected=0)", [id], |row| {
            row.get(0)
        })
        .optional()?;
    bytes.map(|bytes| records::decode(&bytes)).transpose()
}
/// A timer read for someone who asked: a payload past the bound is refused
/// rather than loaded for them.
fn get_timer_bounded(rows: &records::Rows<'_>, id: &str) -> Result<Option<Timer>> {
    use fastctx_storage::rusqlite::OptionalExtension;
    let bytes: Option<Option<Vec<u8>>> = rows
        .connection()
        .query_row("SELECT CASE WHEN length(body)<=16777216 THEN body ELSE NULL END FROM conversation_records r WHERE family='timers' AND identity=?1 AND NOT EXISTS(SELECT 1 FROM node_conversation_transfers t WHERE t.name=r.conversation AND t.rejected=0)", [id], |row| {
            row.get(0)
        })
        .optional()?;
    bytes
        .map(|bytes| {
            let bytes = bytes
                .ok_or_else(|| fastctx_storage::Error::new("a stored timer exceeds the 16777216 byte read limit"))?;
            records::decode(&bytes)
        })
        .transpose()
}
fn delete_timer(rows: &records::Rows<'_>, id: &str) -> Result<bool> {
    Ok(rows.connection().execute("DELETE FROM conversation_records WHERE family='timers' AND identity=?1", [id])? > 0)
}
