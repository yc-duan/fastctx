//! A deadline index in memory; one atomic record per timer on disk.

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use fastctx_platform::doing::{Rank, SyncMutex};
use fastctx_platform::{Permissions, publish};
use fastctx_types::timers::{Action, Owner, Timer};
use fastctx_types::{NoticeId, ResidentId};
use jiff::Timestamp;
use sha2::{Digest, Sha256};
use tokio::sync::Notify;

#[derive(Default)]
struct Index {
    rows: BTreeMap<String, Timer>,
    due: BTreeSet<(Timestamp, String)>,
}

/// Disk changes finish before their in-memory counterpart becomes visible.
/// The mutex covers only local records and is never held across an await.
/// Every row has exactly one due entry, at its `next_attempt`, unless claimed.
pub struct Store {
    directory: PathBuf,
    index: SyncMutex<Index>,
    pub changed: Notify,
}

impl Store {
    /// Reads the table once. An unreadable record is an error, never an absent timer.
    pub fn open(directory: &Path) -> io::Result<Self> {
        fastctx_platform::ensure_private_dir(directory)?;
        let store = Self {
            directory: directory.into(),
            index: SyncMutex::new(Rank::RECORDS, "timer records", directory.display().to_string(), Index::default()),
            changed: Notify::new(),
        };
        let mut index = store.index.lock();
        for entry in std::fs::read_dir(directory)? {
            let path = entry?.path();
            if path.extension().is_none_or(|extension| extension != "json") {
                continue;
            }
            let timer: Timer = serde_json::from_slice(&std::fs::read(&path)?)
                .map_err(|error| io::Error::other(format!("{}: {error}", path.display())))?;
            if store.path(&timer.id) != path {
                return Err(io::Error::other(format!("{}: timer id does not match its file", path.display())));
            }
            index.due.insert((timer.next_attempt, timer.id.clone()));
            index.rows.insert(timer.id.clone(), timer);
        }
        drop(index);
        Ok(store)
    }

    fn path(&self, id: &str) -> PathBuf {
        self.directory.join(format!("{:x}.json", Sha256::digest(id.as_bytes())))
    }

    fn save(&self, timer: &Timer) -> io::Result<()> {
        publish(&self.path(&timer.id), &serde_json::to_vec(timer).map_err(io::Error::other)?, Permissions::Private)
    }

    /// Replaces the same id. Callers serialize this with effects for the owner.
    pub fn set(
        &self,
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
        let mut index = self.index.lock();
        self.save(&timer)?;
        if let Some(old) = index.rows.insert(timer.id.clone(), timer.clone()) {
            index.due.remove(&(old.next_attempt, old.id));
        }
        index.due.insert((due, timer.id.clone()));
        drop(index);
        self.changed.notify_one();
        Ok(timer)
    }

    pub fn get(&self, id: &str) -> Option<Timer> {
        self.index.lock().rows.get(id).cloned()
    }

    pub fn list(&self, owner: Option<&Owner>) -> Vec<Timer> {
        self.index.lock().rows.values().filter(|row| owner.is_none_or(|owner| owner == &row.owner)).cloned().collect()
    }

    pub fn next(&self) -> Option<Timestamp> {
        self.index.lock().due.first().map(|(at, _)| *at)
    }

    /// Reserves one due row in memory. The effect must call `begin` after taking its owner's gate.
    pub fn claim(&self, now: Timestamp) -> Option<Timer> {
        let mut index = self.index.lock();
        loop {
            let (at, id) = index.due.first()?.clone();
            if at > now {
                return None;
            }
            index.due.remove(&(at, id.clone()));
            // An entry of a row replaced since is not that row's turn.
            if let Some(timer) = index.rows.get(&id).filter(|row| row.next_attempt == at) {
                return Some(timer.clone());
            }
        }
    }

    /// Freezes the first firing and records this attempt before any external effect.
    pub fn begin(&self, timer: &Timer, now: Timestamp) -> io::Result<Option<Timer>> {
        let mut index = self.index.lock();
        let Some(row) = index.rows.get(&timer.id).filter(|row| row.delivery == timer.delivery) else { return Ok(None) };
        let mut row = row.clone();
        row.fired_at.get_or_insert(now);
        row.attempts = row.attempts.saturating_add(1);
        self.save(&row)?;
        index.rows.insert(row.id.clone(), row.clone());
        Ok(Some(row))
    }

    /// Keeps a failed effect for the common local backoff. A newer setting is untouched.
    pub fn retry(&self, timer: &Timer, now: Timestamp, error: String) -> io::Result<()> {
        let mut index = self.index.lock();
        let Some(row) = index.rows.get(&timer.id).filter(|row| row.delivery == timer.delivery) else { return Ok(()) };
        let mut row = row.clone();
        row.next_attempt =
            now.checked_add(fastctx_net::again::backoff(row.attempts.max(1))).map_err(io::Error::other)?;
        row.error = Some(error);
        self.save(&row)?;
        index.due.insert((row.next_attempt, row.id.clone()));
        index.rows.insert(row.id.clone(), row);
        drop(index);
        self.changed.notify_one();
        Ok(())
    }

    /// Fires this delivery again at `until` unless its taker completes it first.
    pub fn hold(&self, timer: &Timer, until: Timestamp) -> io::Result<()> {
        let mut index = self.index.lock();
        let Some(row) = index.rows.get(&timer.id).filter(|row| row.delivery == timer.delivery) else { return Ok(()) };
        let mut row = row.clone();
        row.next_attempt = until;
        row.error = None;
        self.save(&row)?;
        index.due.insert((row.next_attempt, row.id.clone()));
        index.rows.insert(row.id.clone(), row);
        drop(index);
        self.changed.notify_one();
        Ok(())
    }

    /// A disk failure must not strand a claimed timer in memory until the next restart.
    pub fn release(&self, timer: &Timer, now: Timestamp) {
        let mut index = self.index.lock();
        if let Some(row) = index.rows.get_mut(&timer.id).filter(|row| row.delivery == timer.delivery) {
            // In memory only, as the disk refused it; the record on disk stays due earlier.
            row.next_attempt = now.checked_add(Duration::from_secs(60)).unwrap_or(now);
            let entry = (row.next_attempt, row.id.clone());
            index.due.insert(entry);
        }
        drop(index);
        self.changed.notify_one();
    }

    /// Removes only this generation after its effect was durably accepted.
    pub fn complete(&self, timer: &Timer) -> io::Result<bool> {
        let mut index = self.index.lock();
        if index.rows.get(&timer.id).is_none_or(|row| row.delivery != timer.delivery) {
            return Ok(false);
        }
        self.remove(&mut index, &timer.id)?;
        drop(index);
        self.changed.notify_one();
        Ok(true)
    }

    pub fn cancel(&self, id: &str) -> io::Result<bool> {
        let mut index = self.index.lock();
        let removed = self.remove(&mut index, id)?;
        drop(index);
        self.changed.notify_one();
        Ok(removed)
    }

    /// Called inside the conversation's durable deletion, before deleting its record.
    pub fn delete_conversation(&self, resident: &ResidentId) -> io::Result<()> {
        let rows = self.list(None);
        for timer in rows {
            if timer.owner.conversation() == Some(resident) {
                self.complete(&timer)?;
            }
        }
        Ok(())
    }

    fn remove(&self, index: &mut Index, id: &str) -> io::Result<bool> {
        if !index.rows.contains_key(id) {
            return Ok(false);
        }
        match std::fs::remove_file(self.path(id)) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
            _ => {}
        }
        index.rows.remove(id);
        index.due.retain(|(_, key)| key != id);
        Ok(true)
    }
}
