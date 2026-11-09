//! Executor replies read a committed, bounded step view. Journal replay belongs
//! to recording and recovery; a reply never opens a step segment or its lock.
use super::{WRITING, journal, sql};
use crate::job_metadata::Job;
use fastctx_storage::Result;
use fastctx_types::run_facts::{RunStep, StepSummary};
use serde::{Deserialize, Serialize};
use std::{io, path::Path};

const VIEW: &str = "step-view";
const CURSOR: &str = "step-view-cursor";
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Cursor {
    segment: u64,
    offset: u64,
}

/// Replies read the view, but the journal it is built from stays pinned until
/// it catches up, so a failure here never holds back a step or a job's facts.
pub(super) fn report(node: &crate::Node, result: io::Result<()>) {
    static LAST: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
    if let Err(error) = result {
        fastctx_platform::readings::counter("run_facts.step_view_failures").add(1);
        let reason = error.to_string();
        let mut last = LAST.lock().expect("step view failure");
        if last.as_deref() != Some(reason.as_str()) {
            node.log.line(format_args!("refreshing a job's step view: {reason}"));
            *last = Some(reason);
        }
    }
}

pub(super) fn refresh(record: &Path) -> io::Result<()> {
    let gate = WRITING.get(&record.with_extension("facts"));
    let _writing = gate.lock();
    refresh_locked(record)
}

pub(super) fn refresh_locked(record: &Path) -> io::Result<()> {
    let job = Job::open(record)?;
    let mut cursor = match job.read(CURSOR, |rows, family| rows.get::<Cursor>(family, "value"))? {
        Some(cursor) => cursor,
        None => Cursor { segment: journal::first(record)?, offset: 0 },
    };
    loop {
        let before = cursor.clone();
        let steps = journal::read_since(record, &mut cursor.segment, &mut cursor.offset)?;
        if steps.is_empty() && cursor == before {
            // An empty job still has a prepared view, so replies need no file probe.
            if job.read(CURSOR, |rows, family| rows.get::<Cursor>(family, "value"))?.is_none() {
                job.change(CURSOR, 128, move |rows, family| rows.put(family, "value", &cursor))?;
            }
            return Ok(());
        }
        let count = steps.len().div_ceil(16).max(1);
        for index in 0..count {
            let steps = steps.iter().skip(index * 16).take(16).cloned().collect::<Vec<_>>();
            let bytes = serde_json::to_vec(&steps)?.len();
            let cursor_family = job.family(CURSOR);
            let committed = (index + 1 == count).then(|| cursor.clone());
            job.change(VIEW, bytes + 128, move |rows, family| {
                sql::steps(rows.connection(), rows.conversation(), family, &steps)?;
                if let Some(cursor) = committed {
                    rows.put(&cursor_family, "value", &cursor)?;
                }
                Ok(())
            })?;
        }
        if steps.is_empty() {
            return Ok(());
        }
    }
}

pub(super) fn read(record: &Path) -> io::Result<(Vec<RunStep>, Vec<StepSummary>)> {
    let job = Job::open(record)?;
    let cursor_family = job.family(CURSOR);
    job.read(VIEW, |rows, family| {
        if !rows.exists(&cursor_family, "value")? {
            return Err(fastctx_storage::Error::with_kind(fastctx_storage::ErrorKind::Busy, "the executor's step view is still being prepared"));
        }
        let mut query = rows.connection().prepare_cached("SELECT body FROM run_steps INDEXED BY run_steps_hot WHERE conversation=?1 AND call=?2 AND body IS NOT NULL ORDER BY position")?;
        let steps = query.query_map((rows.conversation(), family), |row| row.get::<_, Vec<u8>>(0))?
            .map(|row| crate::records::decode(&row?)).collect::<Result<Vec<RunStep>>>()?;
        let mut query = rows.connection().prepare_cached("SELECT machine,kind,count,first,last FROM run_folded WHERE conversation=?1 AND call=?2 ORDER BY machine,kind")?;
        let folded = query.query_map((rows.conversation(), family), |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, i64>(2)?, row.get::<_, i64>(3)?, row.get::<_, i64>(4)?))
        })?.map(|row| {
            let (machine, kind, count, first, last) = row?;
            Ok(StepSummary {
                machine: machine.parse().map_err(|error: fastctx_types::names::NameError| fastctx_storage::Error::new(error.to_string()))?,
                kind, count: count as u64,
                first: jiff::Timestamp::from_microsecond(first).map_err(|error| fastctx_storage::Error::new(error.to_string()))?,
                last: jiff::Timestamp::from_microsecond(last).map_err(|error| fastctx_storage::Error::new(error.to_string()))?,
            })
        }).collect::<Result<Vec<_>>>()?;
        Ok((steps, folded))
    })
}
