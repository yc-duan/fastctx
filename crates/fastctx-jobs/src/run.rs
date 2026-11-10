//! Running jobs: each kind takes its due jobs a few at once, one of a unit
//! at a time, and records how each run ended.
//!
//! Taking a job and recording its end do not wait for the disk. A machine
//! that loses its power then finds the job as it was asked for, which
//! reached the disk with what asked for it, and runs it again: a job's work
//! is done again unharmed, as it is after any failure.

use crate::table::{ASIDE, RUNNING, WAITING, now};
use crate::{Jobs, Outcome, Registered, Taken};
use fastctx_storage::rusqlite::{Connection, OptionalExtension, params};
use fastctx_storage::{Database, Result};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// The first wait after a failure; it doubles with each failure in a row.
const BACKOFF: Duration = Duration::from_millis(250);
/// The longest wait between tries.
const BACKOFF_AT_MOST: Duration = Duration::from_secs(60);
/// How long a runner with nothing due sleeps when no one stirs it.
const IDLE: Duration = Duration::from_secs(30);
/// How many due jobs are looked at, at most, for those whose units are free.
const LOOKED_AT: usize = 256;
/// Bytes a bookkeeping write retains, beyond a payload.
const WRITE_BYTES: usize = 512;

struct Next {
    id: i64,
    taken: Taken,
}

/// The jobs taken, and when the earliest job not yet due is, when none was taken.
type Looked = (Vec<Next>, Option<i64>);

impl Jobs {
    /// Starts running jobs, on the runtime this is called in. Jobs a stopped
    /// process left running are waiting again first.
    pub async fn start(self: &Arc<Self>) -> Result<()> {
        assert!(!self.started.swap(true, Ordering::AcqRel), "jobs are started once");
        self.database
            .write(WRITE_BYTES, |db| {
                db.execute("UPDATE jobs SET state = ?1 WHERE state = ?2", params![WAITING, RUNNING])?;
                Ok(())
            })
            .await?;
        let kinds: Vec<_> = self.kinds.lock().expect("job kinds").values().cloned().collect();
        for registered in kinds {
            fastctx_platform::task::spawn(serve(self.database.clone(), registered));
        }
        Ok(())
    }
}

async fn serve(database: Database, registered: Arc<Registered>) {
    let places = Arc::new(Semaphore::new(registered.kind.at_once));
    loop {
        let first = places.clone().acquire_owned().await.expect("the places are never closed");
        // Only this loop takes places, so those free now stay free until it does.
        let most = 1 + places.available_permits();
        let looked = {
            let registered = registered.clone();
            database.write_relaxed(WRITE_BYTES, move |db| look(db, &registered, most)).await
        };
        match looked {
            Ok((taken, due)) if taken.is_empty() => {
                drop(first);
                let wait = match due {
                    Some(due) => Duration::from_millis((due - now()).clamp(1, IDLE.as_millis() as i64) as u64),
                    None => IDLE,
                };
                let _ = tokio::time::timeout(wait, registered.stirred.notified()).await;
            }
            Ok((taken, _)) => {
                let mut first = Some(first);
                for next in taken {
                    let place = match first.take() {
                        Some(place) => place,
                        None => places.clone().acquire_owned().await.expect("the places are never closed"),
                    };
                    fastctx_platform::task::spawn(run(database.clone(), registered.clone(), next, place));
                }
            }
            Err(_) => {
                drop(first);
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    }
}

/// Takes the earliest due jobs, `most` at most, each of a unit no job of the
/// kind runs for.
fn look(db: &Connection, registered: &Registered, most: usize) -> Result<Looked> {
    let kind = registered.kind.name;
    let due: Vec<(i64, String, String, Vec<u8>, u32)> = db
        .prepare_cached(
            "SELECT id, key, unit, payload, tries FROM jobs \
             WHERE kind = ?1 AND state = ?2 AND due <= ?3 ORDER BY due, id LIMIT ?4",
        )?
        .query_map(params![kind, WAITING, now(), (most * 8).min(LOOKED_AT) as i64], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?))
        })?
        .collect::<fastctx_storage::rusqlite::Result<_>>()?;
    let mut taken = Vec::new();
    {
        let mut busy = registered.busy.lock().expect("busy units");
        let mut take = db.prepare_cached("UPDATE jobs SET state = ?2, tries = tries + 1 WHERE id = ?1")?;
        for (id, key, unit, payload, tries) in due {
            if taken.len() == most {
                break;
            }
            if busy.contains(&unit) {
                continue;
            }
            take.execute(params![id, RUNNING])?;
            busy.insert(unit.clone());
            taken.push(Next { id, taken: Taken { key, unit, payload, attempt: tries + 1 } });
        }
    }
    if !taken.is_empty() {
        return Ok((taken, None));
    }
    let earliest: Option<i64> = db
        .prepare_cached("SELECT MIN(due) FROM jobs WHERE kind = ?1 AND state = ?2 AND due > ?3")?
        .query_row(params![kind, WAITING, now()], |row| row.get(0))?;
    Ok((taken, earliest))
}

async fn run(database: Database, registered: Arc<Registered>, next: Next, place: OwnedSemaphorePermit) {
    let Next { id, taken } = next;
    let (unit, attempt) = (taken.unit.clone(), taken.attempt);
    let kind = registered.kind;
    let outcome = match tokio::time::timeout(kind.within, (registered.run)(taken)).await {
        Ok(outcome) => outcome,
        Err(_) => Outcome::Failed(format!("{} did not finish within {:?}", kind.name, kind.within)),
    };
    // A run's end that is not recorded leaves the job running until the
    // process starts again, so it is written until it is.
    let mut wait = BACKOFF;
    loop {
        let outcome = outcome.clone();
        match database.write_relaxed(WRITE_BYTES, move |db| finish(db, id, &outcome, attempt, kind.tries)).await {
            Ok(()) => break,
            Err(_) => {
                tokio::time::sleep(wait).await;
                wait = (wait * 2).min(BACKOFF_AT_MOST);
            }
        }
    }
    registered.busy.lock().expect("busy units").remove(&unit);
    drop(place);
    registered.stirred.notify_one();
}

/// Records how a run ended. What was asked for while it ran comes first: the
/// job waits again with that payload, whatever the run came to.
fn finish(db: &Connection, id: i64, outcome: &Outcome, attempt: u32, tries: u32) -> Result<()> {
    let asked: Option<Option<Vec<u8>>> =
        db.prepare_cached("SELECT again FROM jobs WHERE id = ?1")?.query_row([id], |row| row.get(0)).optional()?;
    let Some(asked) = asked else { return Ok(()) };
    if let Some(payload) = asked {
        db.prepare_cached(
            "UPDATE jobs SET payload = ?2, again = NULL, state = ?3, due = ?4, tries = 0, failure = NULL \
             WHERE id = ?1",
        )?
        .execute(params![id, payload, WAITING, now()])?;
        return Ok(());
    }
    match outcome {
        Outcome::Done => {
            db.prepare_cached("DELETE FROM jobs WHERE id = ?1")?.execute([id])?;
        }
        Outcome::Again { after } => {
            db.prepare_cached("UPDATE jobs SET state = ?2, due = ?3, tries = 0, failure = NULL WHERE id = ?1")?
                .execute(params![id, WAITING, now() + after.as_millis() as i64])?;
        }
        Outcome::Failed(reason) if attempt >= tries => {
            db.prepare_cached("UPDATE jobs SET state = ?2, failure = ?3 WHERE id = ?1")?
                .execute(params![id, ASIDE, reason])?;
        }
        Outcome::Failed(reason) => {
            let wait = BACKOFF.saturating_mul(1u32 << (attempt - 1).min(16)).min(BACKOFF_AT_MOST);
            db.prepare_cached("UPDATE jobs SET state = ?2, due = ?3, failure = ?4 WHERE id = ?1")?.execute(params![
                id,
                WAITING,
                now() + wait.as_millis() as i64,
                reason
            ])?;
        }
    }
    Ok(())
}
