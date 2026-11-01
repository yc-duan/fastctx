//! One published page per job. Frames are staged independently; a small manifest
//! and the source cursors commit together. An interrupted stage is discarded on retry.
use super::updates::Emitted;
use crate::{job_metadata::Job, record_payloads as payloads};
use fastctx_types::run_facts::JobFacts;
use serde::{Deserialize, Serialize};
use std::io;

static STAGED: std::sync::LazyLock<&'static fastctx_platform::readings::Counter> =
    std::sync::LazyLock::new(|| fastctx_platform::readings::counter("node.facts.frames.staged"));
static SENT: std::sync::LazyLock<&'static fastctx_platform::readings::Counter> =
    std::sync::LazyLock::new(|| fastctx_platform::readings::counter("node.facts.frames.sent"));
static BYTES: std::sync::LazyLock<&'static fastctx_platform::readings::Counter> =
    std::sync::LazyLock::new(|| fastctx_platform::readings::counter("node.facts.frames.bytes"));

const QUEUE: &str = "facts-queue";
const PARTS: &str = "facts-frames";
const MAX_PARTS: usize = 4096;

#[derive(Serialize, Deserialize)]
struct Queue {
    next: u64,
    through: u64,
}

pub(super) fn publish(job: &Job, update: JobFacts, emitted: &mut Emitted) -> io::Result<()> {
    clear_staging(job)?;
    let frames = super::frames::split(update)?;
    if frames.len() > MAX_PARTS {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "one fact page exceeds its 4096-frame budget"));
    }
    let count = frames.len();
    let state = job.family("facts-state");
    let legacy = job.family("facts-pending");
    let mut first = None;
    for mut frame in frames {
        frame.sequence = job.next_sequence(emitted.sequence)?;
        emitted.sequence = frame.sequence;
        first.get_or_insert(frame.sequence);
        let key = format!("{:020}", frame.sequence);
        let bytes = serde_json::to_vec(&frame)?;
        BYTES.add(bytes.len() as u64);
        let staged = job.stage(bytes)?;
        job.change(PARTS, 512, move |rows, family| {
            payloads::link(rows.connection(), rows.conversation(), family, &key, Some(&staged.reference))?;
            rows.put(family, &key, &staged.reference)?;
            payloads::finish(rows.connection(), rows.conversation(), &staged)
        })?;
    }
    STAGED.add(count as u64);
    let queue = Queue { next: first.expect("one final fact frame"), through: emitted.sequence };
    let emitted = emitted.clone();
    job.change(QUEUE, 1024, move |rows, family| {
        if rows.get::<Queue>(family, "value")?.is_some() {
            return Err(fastctx_storage::Error::new(
                "an earlier fact publication completed after its caller stopped waiting",
            ));
        }
        rows.put(&state, "value", &emitted)?;
        rows.delete(&legacy, "value")?;
        rows.put(family, "value", &queue)
    })
}

pub(super) fn drain(job: &Job, mut send: impl FnMut(&JobFacts) -> io::Result<()>) -> io::Result<()> {
    let parts = job.family(PARTS);
    loop {
        let next = job.read(QUEUE, |rows, family| {
            let Some(queue) = rows.get::<Queue>(family, "value")? else { return Ok(None) };
            let key = format!("{:020}", queue.next);
            let bytes = payloads::read_root(rows.connection(), rows.conversation(), &parts, &key)?
                .ok_or_else(|| fastctx_storage::Error::new("pending fact frame is missing"))?;
            Ok(Some((queue, crate::records::decode::<JobFacts>(&bytes)?)))
        })?;
        let Some((queue, frame)) = next else { return Ok(()) };
        send(&frame)?;
        SENT.add(1);
        let parts = parts.clone();
        job.change(QUEUE, 512, move |rows, family| {
            let key = format!("{:020}", queue.next);
            rows.delete(&parts, &key)?;
            payloads::link(rows.connection(), rows.conversation(), &parts, &key, None)?;
            if queue.next == queue.through {
                rows.delete(family, "value")?;
            } else {
                // Sequence allocation can have gaps after an interrupted stage.
                let next: Option<String> = rows.connection().query_row(
                    "SELECT min(identity) FROM conversation_records WHERE conversation=?1 AND family=?2 AND identity>?3",
                    (rows.conversation(), &parts, &key), |row| row.get(0),
                )?;
                let next = next.ok_or_else(|| fastctx_storage::Error::new("pending fact queue is incomplete"))?
                    .parse().map_err(|_| fastctx_storage::Error::new("invalid fact sequence"))?;
                rows.put(family, "value", &Queue { next, ..queue })?;
            }
            Ok(())
        })?;
    }
}

fn clear_staging(job: &Job) -> io::Result<()> {
    if job.read(QUEUE, |rows, family| rows.get::<Queue>(family, "value"))?.is_some() {
        return Err(io::Error::other("pending fact frames must drain before staging another page"));
    }
    loop {
        let queue = job.family(QUEUE);
        let count = job.change(PARTS, 4096, move |rows, family| {
            // The writer may have committed a timed-out publication after the
            // caller's read. Never delete frames owned by that durable queue.
            if rows.get::<Queue>(&queue, "value")?.is_some() {
                return Err(fastctx_storage::Error::new("published fact frames still need delivery"));
            }
            let mut query = rows.connection().prepare_cached("SELECT identity FROM conversation_records WHERE conversation=?1 AND family=?2 ORDER BY identity LIMIT 16")?;
            let keys = query.query_map((rows.conversation(), family), |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            for key in &keys {
                rows.delete(family, key)?;
                payloads::link(rows.connection(), rows.conversation(), family, key, None)?;
            }
            Ok(keys.len())
        })?;
        if count == 0 {
            return Ok(());
        }
    }
}
