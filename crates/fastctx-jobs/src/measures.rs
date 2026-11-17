//! The queue's measures of itself (`fastctx_platform::readings`), by kind:
//! how long a job waited past its time before it was taken, how long a run
//! took, the runs that failed and the jobs set aside; and, as the table
//! stands when asked (`Jobs::measure`), how many wait, run and are set
//! aside, and how long the oldest due job has waited.

use crate::table::{ASIDE, RUNNING, WAITING, now};
use crate::{Jobs, Registered};
use fastctx_platform::readings::{Counter, Gauge, Timing, counter, gauge, timing};
use fastctx_storage::Result;
use fastctx_storage::rusqlite::params;
use std::collections::HashMap;
use std::sync::Arc;

pub(crate) struct Measures {
    pub waited: &'static Timing,
    pub ran: &'static Timing,
    pub failed: &'static Counter,
    pub set_aside: &'static Counter,
    waiting: &'static Gauge,
    running: &'static Gauge,
    aside: &'static Gauge,
    oldest_ms: &'static Gauge,
}

impl Measures {
    /// The measures of a kind, named `job.<measure>.<kind>`. Kinds are few
    /// and named in the code, so their names are kept for the process.
    pub(crate) fn of(kind: &'static str) -> Self {
        let named = |measure: &str| -> &'static str { Box::leak(format!("job.{measure}.{kind}").into_boxed_str()) };
        Self {
            waited: timing(named("waited")),
            ran: timing(named("ran")),
            failed: counter(named("failed")),
            set_aside: counter(named("set_aside")),
            waiting: gauge(named("waiting")),
            running: gauge(named("running")),
            aside: gauge(named("aside")),
            oldest_ms: gauge(named("oldest_ms")),
        }
    }
}

impl Jobs {
    /// Brings the measures of what the table holds up to now: for whoever
    /// is about to read the process's readings.
    pub async fn measure(&self) -> Result<()> {
        let at = now();
        let rows: Vec<(String, i64, i64, Option<i64>)> = self
            .database
            .read(move |db| {
                let mut counted = db.prepare_cached(
                    "SELECT kind, state, COUNT(*), MIN(CASE WHEN due <= ?1 THEN due END) FROM jobs GROUP BY kind, state",
                )?;
                let rows = counted
                    .query_map(params![at], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))?
                    .collect::<fastctx_storage::rusqlite::Result<_>>()?;
                Ok(rows)
            })
            .await?;
        let mut held: HashMap<(String, i64), (i64, Option<i64>)> = HashMap::new();
        for (kind, state, count, due) in rows {
            held.insert((kind, state), (count, due));
        }
        let kinds: Vec<Arc<Registered>> = self.kinds.lock().expect("job kinds").values().cloned().collect();
        for registered in kinds {
            let of = |state: i64| held.get(&(registered.kind.name.to_string(), state)).copied().unwrap_or((0, None));
            let (waiting, due) = of(WAITING);
            registered.measures.waiting.set(waiting);
            registered.measures.running.set(of(RUNNING).0);
            registered.measures.aside.set(of(ASIDE).0);
            registered.measures.oldest_ms.set(due.map_or(0, |due| (at - due).max(0)));
        }
        Ok(())
    }
}
