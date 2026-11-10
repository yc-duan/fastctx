//! Operations: what a call that submits work is answered with, and how
//! whoever asked learns what came of it.
//!
//! A submitting call names its operation. In its one transaction it accepts
//! the operation and submits the job that does it, and is answered at once:
//! accepted. Asked again under the same name, whether for an answer that was
//! lost or by a second device, it is not done twice: the call is answered
//! with how the operation stands. The job settles the operation when its
//! work is done or cannot be, and those who wait for it are told. How an
//! operation stands is kept as a receipt of the service's database for a
//! while after it settles.

use crate::Jobs;
use fastctx_storage::rusqlite::Connection;
use fastctx_storage::{Result, receipts};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::watch;

/// The largest result an operation is settled with; a larger one is named
/// by where it is.
pub const RESULT_BYTES: usize = 64 * 1024;
/// Past this many operations waited for, those no one waits for are let go.
const KEPT: usize = 4096;

const ACCEPTED: u8 = b'A';
const DONE: u8 = b'D';
const FAILED: u8 = b'F';

/// One operation: what kind of thing it is, the unit it is done for, and
/// the name its asker gave it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Operation {
    pub kind: &'static str,
    pub unit: String,
    pub id: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Standing {
    /// Taken, and not yet done.
    Accepted,
    /// Done, with what it came to.
    Done(Vec<u8>),
    /// It could not be done, and why.
    Failed(String),
}

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_millis() as i64)
}

fn key(operation: &Operation) -> receipts::Key {
    receipts::Key::new(format!("operation/{}", operation.kind), operation.unit.clone(), operation.id.clone())
}

fn read(value: &[u8]) -> Standing {
    match value.split_first() {
        Some((&DONE, result)) => Standing::Done(result.to_vec()),
        Some((&FAILED, why)) => Standing::Failed(String::from_utf8_lossy(why).into_owned()),
        _ => Standing::Accepted,
    }
}

/// How an operation stands; none when it was never accepted, or settled so
/// long ago that it is no longer kept.
pub fn standing(db: &Connection, operation: &Operation) -> Result<Option<Standing>> {
    Ok(receipts::get(db, &key(operation))?.map(|value| read(&value)))
}

/// Accepts an operation in the transaction that submits its job: none when
/// it is new, and is accepted by this; how it stands when it was accepted
/// before, in which case nothing is to be submitted.
pub fn accept(db: &Connection, operation: &Operation) -> Result<Option<Standing>> {
    if let Some(known) = standing(db, operation)? {
        return Ok(Some(known));
    }
    // Kept until it settles: an operation under way has no age.
    receipts::put(db, &key(operation), &[ACCEPTED], Duration::ZERO, false, None, now())?;
    Ok(None)
}

/// Settles an operation in the transaction that records what its work did,
/// keeping how it stands for `kept` from now. `Jobs::told` then tells those
/// who wait, once the transaction is committed.
pub fn settle(db: &Connection, operation: &Operation, standing: &Standing, kept: Duration) -> Result<()> {
    let value = match standing {
        Standing::Accepted => vec![ACCEPTED],
        Standing::Done(result) if result.len() > RESULT_BYTES => {
            return Err(fastctx_storage::Error::with_kind(
                fastctx_storage::ErrorKind::Invalid,
                format!(
                    "an operation settles with at most {RESULT_BYTES} bytes; a larger result is named by where it is"
                ),
            ));
        }
        Standing::Done(result) => [&[DONE][..], result].concat(),
        Standing::Failed(why) => [&[FAILED][..], why.as_bytes()].concat(),
    };
    let settled = !matches!(standing, Standing::Accepted);
    receipts::put(db, &key(operation), &value, kept, settled, None, now())
}

impl Jobs {
    /// Tells those who wait for an operation that it has settled.
    pub fn told(&self, operation: &Operation) {
        let mut waited = self.waited.lock().expect("operations waited for");
        if let Some(waiting) = waited.get(operation) {
            if waiting.receiver_count() == 0 {
                waited.remove(operation);
            } else {
                waiting.send_modify(|settled| *settled += 1);
            }
        }
    }

    /// How an operation stands once it has settled, or as it stands after
    /// `within`: for a caller that waits a bounded while and asks again.
    pub async fn settled(&self, operation: &Operation, within: Duration) -> Result<Option<Standing>> {
        // Watched before it is read, so a settling between the two is not missed.
        let mut watching = {
            let mut waited = self.waited.lock().expect("operations waited for");
            if waited.len() >= KEPT {
                waited.retain(|_, waiting| waiting.receiver_count() > 0);
            }
            waited.entry(operation.clone()).or_insert_with(|| watch::channel(0u64).0).subscribe()
        };
        let asked = operation.clone();
        let read = move |db: &Connection| standing(db, &asked);
        let mut stands = self.database.read(read.clone()).await?;
        if !matches!(stands, Some(Standing::Accepted)) {
            return Ok(stands);
        }
        if tokio::time::timeout(within, watching.changed()).await.is_ok() {
            stands = self.database.read(read).await?;
        }
        Ok(stands)
    }
}
