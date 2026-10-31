//! SQL owns timer generations and their due index. Only active claims and a
//! disk-failure backoff live in memory; no registry lock covers storage work.

use crate::records::{self, Source};
use fastctx_storage::{Priority, Result};
use fastctx_types::{
    NoticeId, ResidentId,
    layout::ConversationDir,
    timers::{Action, Owner, Timer},
};
use jiff::Timestamp;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    io,
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::Notify;

const FAMILY: &str = "timers";
#[derive(Default, Clone)]
struct Claims {
    active: HashMap<String, NoticeId>,
    delayed: HashMap<(String, NoticeId), Timestamp>,
}
pub struct Store {
    directory: ConversationDir,
    claims: Mutex<Claims>,
    pub changed: Notify,
}
impl Store {
    pub fn open(path: &Path) -> io::Result<Self> {
        let (directory, _, _, _) = crate::record_database::item(&path.join(".timers"))?;
        records::ensure_each(&directory, FAMILY, |push| {
            let entries = match std::fs::read_dir(path) {
                Ok(entries) => entries,
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
                Err(error) => return Err(error),
            };
            for entry in entries {
                let source = entry?.path();
                if source.extension().is_none_or(|ext| ext != "json") {
                    continue;
                }
                let timer: Timer = serde_json::from_slice(&std::fs::read(&source)?)?;
                if source != path.join(format!("{:x}.json", Sha256::digest(timer.id.as_bytes()))) {
                    return Err(io::Error::other("timer id does not match its file"));
                }
                push(
                    Source::new(timer.id.clone(), source, serde_json::to_vec(&timer)?)?
                        .ordered(timer.next_attempt)
                        .categorized(serde_json::to_string(&timer.owner)?),
                )?;
            }
            Ok(())
        })?;
        Ok(Self { directory, claims: Mutex::new(Claims::default()), changed: Notify::new() })
    }
    async fn work<T: Send + 'static>(
        self: &Arc<Self>,
        action: impl FnOnce(&Self) -> io::Result<T> + Send + 'static,
    ) -> io::Result<T> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || action(&store)).await.map_err(io::Error::other)?
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
                    if rows.get::<Timer>(FAMILY, &timer.id)?.is_some_and(|old| old.owner != timer.owner) {
                        return Err(fastctx_storage::Error::new("cancel a timer before changing its owner"));
                    }
                    save(rows, &timer)?;
                    Ok(timer)
                },
            )?;
            store.changed.notify_one();
            Ok(result)
        })
        .await
    }
    pub async fn get(self: &Arc<Self>, id: &str) -> io::Result<Option<Timer>> {
        let id = id.to_string();
        self.work(move |store| records::read(&store.directory, |rows| rows.get(FAMILY, &id))).await
    }
    pub async fn list(self: &Arc<Self>, owner: Option<&Owner>) -> io::Result<Vec<Timer>> {
        let owner = owner.cloned();
        self.work(move |store| store.list_now(owner.as_ref())).await
    }
    fn list_now(&self, owner: Option<&Owner>) -> io::Result<Vec<Timer>> {
        let category = owner.map(serde_json::to_string).transpose()?;
        records::read(&self.directory, |rows| {
            let mut query=rows.connection().prepare_cached("SELECT body FROM conversation_records WHERE conversation=?1 AND family='timers' AND (?2 IS NULL OR category=?2) ORDER BY identity")?;
            query
                .query_map((rows.conversation(), category), |row| row.get::<_, Vec<u8>>(0))?
                .map(|row| records::decode(&row?))
                .collect()
        })
    }
    fn candidate(&self) -> io::Result<Option<(Timestamp, Timer)>> {
        let claims = self.claims.lock().expect("timer claims").clone();
        // Only entries excluded by the small live claim/backoff set need to be
        // skipped. The next unexcluded row bounds every later candidate.
        let limit = claims.active.len() + claims.delayed.len() + 1;
        let rows = records::read(&self.directory, |rows| {
            let mut query=rows.connection().prepare_cached("SELECT body FROM conversation_records WHERE conversation=?1 AND family='timers' ORDER BY arrival,identity LIMIT ?2")?;
            query
                .query_map((rows.conversation(), limit as i64), |row| row.get::<_, Vec<u8>>(0))?
                .map(|row| records::decode::<Timer>(&row?))
                .collect::<Result<Vec<_>>>()
        })?;
        Ok(rows
            .into_iter()
            .filter(|row| !claims.active.contains_key(&row.id))
            .map(|row| {
                let due = claims
                    .delayed
                    .get(&(row.id.clone(), row.delivery.clone()))
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
                if claims.active.contains_key(&timer.id) {
                    continue;
                }
                claims.active.insert(timer.id.clone(), timer.delivery.clone());
                claims.delayed.remove(&(timer.id.clone(), timer.delivery.clone()));
                return Ok(Some(timer));
            }
        })
        .await
    }
    pub fn unclaim(&self, timer: &Timer) {
        let mut claims = self.claims.lock().expect("timer claims");
        if claims.active.get(&timer.id) == Some(&timer.delivery) {
            claims.active.remove(&timer.id);
        }
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
                let Some(mut timer) = rows.get::<Timer>(FAMILY, &id)?.filter(|row| row.delivery == delivery) else {
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
        self.claims.lock().expect("timer claims").delayed.insert(
            (timer.id.clone(), timer.delivery.clone()),
            now.checked_add(Duration::from_secs(60)).unwrap_or(now),
        );
        self.unclaim(timer);
    }
    pub async fn complete(self: &Arc<Self>, timer: &Timer) -> io::Result<bool> {
        let (id, delivery) = (timer.id.clone(), timer.delivery.clone());
        self.work(move |store| {
            let removed = records::write_at(&store.directory, Priority::Interactive, 128, move |rows| {
                if rows.get::<Timer>(FAMILY, &id)?.is_none_or(|row| row.delivery != delivery) {
                    return Ok(false);
                }
                rows.delete(FAMILY, &id)
            })?;
            store.changed.notify_one();
            Ok(removed)
        })
        .await
    }
    pub async fn cancel(self: &Arc<Self>, id: &str) -> io::Result<bool> {
        let id = id.to_string();
        self.work(move |store| {
            let removed =
                records::write_at(&store.directory, Priority::Control, 128, move |rows| rows.delete(FAMILY, &id))?;
            store.changed.notify_one();
            Ok(removed)
        })
        .await
    }
    pub fn delete_conversation(&self, resident: &ResidentId) -> io::Result<()> {
        for owner in [Owner::Conversation { resident: resident.clone() }, Owner::Goal { resident: resident.clone() }] {
            let category = serde_json::to_string(&owner)?;
            loop {
                let category = category.clone();
                let count = records::write_at(&self.directory, Priority::Background, 4096, move |rows| {
                    Ok(rows.connection().execute("DELETE FROM conversation_records WHERE position IN (SELECT position FROM conversation_records WHERE conversation=?1 AND family='timers' AND category=?2 LIMIT 64)",(rows.conversation(),category))?)
                })?;
                if count == 0 {
                    break;
                }
            }
        }
        self.changed.notify_one();
        Ok(())
    }
}
fn save(rows: &records::Rows<'_>, timer: &Timer) -> Result<()> {
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
