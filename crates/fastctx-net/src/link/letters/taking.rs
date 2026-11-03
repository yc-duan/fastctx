//! A ready receive window is processed in order and its receipts commit together.
//! The owned receive lease outlives a disconnected session, including publication
//! of its cumulative position; a replacement session cannot overtake this work.

use super::{Receipt, receipts};
use crate::envelope::{Envelope, Kind};
use crate::link::Inner;
use crate::message::{LetterBody, LetterId};
use crate::sign::{Digest, PublicKey, sha256};
use fastctx_storage::receipts::Reference;
use jiff::Timestamp;
use std::collections::HashMap;
use std::io;
use std::sync::Arc;

/// A run not taken: the letter it stopped at, where that letter's own
/// handling failed, and why.
pub(in crate::link) struct Untaken {
    pub at: Option<u64>,
    pub error: io::Error,
}

impl From<io::Error> for Untaken {
    fn from(error: io::Error) -> Self {
        Self { at: None, error }
    }
}

/// Takes `run`; the letter numbered `give_up`, where its handling fails in
/// a way taking it again does not mend (`lasting`), is set aside instead.
pub(in crate::link) async fn take(
    inner: Arc<Inner>,
    epoch: u64,
    run: Arc<[(u64, Envelope)]>,
    give_up: Option<u64>,
) -> Result<u64, Untaken> {
    let held = inner.taking.lock_owned().await;
    fastctx_platform::task::spawn(async move {
        let _doing = fastctx_platform::doing::doing(format!("persisting letters from {}", inner.hub));
        held.adopt();
        let _held = held;
        let taken = inner.letters.lock().expect("never poisoned").file.taken;
        let mut pending = Vec::new();
        let mut recent = HashMap::new();
        let mut last = None;
        for &(sequence, ref message) in run.iter() {
            if crate::window::unheld(taken, epoch, sequence) {
                let reference = Reference { authority: inner.hub.as_str().to_owned(), epoch, sequence };
                match one(&inner, message, &mut recent, &reference, give_up == Some(sequence)).await {
                    Ok(Some((from, id, receipt))) => pending.push((from, id, receipt, reference)),
                    Ok(None) => {}
                    Err(error) => {
                        // Earlier business handoffs in this window still need
                        // their receipts, even when a later handoff must retry.
                        receipts::keep_batch(&inner.dir, pending).await?;
                        return Err(Untaken { at: Some(sequence), error });
                    }
                }
            }
            last = Some(sequence);
        }
        receipts::keep_batch(&inner.dir, pending).await?;
        let sequence = last.expect("receive windows are nonempty");
        let keeping = inner.clone();
        inner
            .blocking(move || {
                keeping.change_letters(|letters| {
                    let further = crate::window::unheld(letters.file.taken, epoch, sequence);
                    if further {
                        letters.file.taken = Some((epoch, sequence));
                    }
                    further
                })
            })
            .await
            .map_err(io::Error::other)??;
        Ok(sequence)
    })
    .await
    .map_err(|error| Untaken::from(io::Error::other(error)))?
}

/// Whether a letter's handling failed for what the letter is, or what it is
/// for, rather than for this machine's storage: taking it again does not
/// mend it.
fn lasting(error: &io::Error) -> bool {
    matches!(error.kind(), io::ErrorKind::NotFound | io::ErrorKind::InvalidData | io::ErrorKind::InvalidInput)
}

/// Admission and business errors are set aside durably. Local storage failures
/// leave the window unacknowledged so the hub delivers it again; with
/// `give_up`, a lasting failure of the letter's own handling is set aside too.
async fn one(
    inner: &Inner,
    message: &Envelope,
    recent: &mut HashMap<(PublicKey, LetterId), Receipt>,
    reference: &Reference,
    give_up: bool,
) -> io::Result<Option<(PublicKey, LetterId, Receipt)>> {
    let set_aside = |reason: String| -> io::Result<()> {
        let path = super::record_failure(&inner.dir, "in", &reason, message, |line| inner.node.log(line))?;
        inner.node.log(format!("a letter was set aside: {reason}; kept in {}", path.display()));
        Ok(())
    };
    if message.kind != Kind::Letter {
        set_aside(format!("from {}: it is not a letter", message.from))?;
        return Ok(None);
    }
    // The end of a job an earlier release ran to repair itself is told to no
    // one (`fastctx_types::repair::JOBS`): it is taken and kept nowhere.
    if message.body.pointer("/letter/to").and_then(serde_json::Value::as_str) == Some(fastctx_types::repair::JOBS)
        && message.body.pointer("/letter/notice/kind").and_then(serde_json::Value::as_str) == Some("job_ended")
    {
        return Ok(None);
    }
    let mut caller = match inner.admit(message).await {
        Ok(caller) => caller,
        Err(reason) => {
            set_aside(format!("from {}: {reason}", message.from))?;
            return Ok(None);
        }
    };
    let purpose = message.purpose.expect("an admitted letter names its purpose");
    let LetterBody { id, letter } = match message.body() {
        Ok(body) => body,
        Err(error) => {
            set_aside(format!("from {}: {error}", message.from))?;
            return Ok(None);
        }
    };
    let digest = Digest(sha256(&serde_json::to_vec(&letter).expect("letters serialize")));
    let sender = caller.key;
    let previous = match recent.get(&(sender, id.clone())) {
        Some(receipt) => Some(*receipt),
        None => receipts::receipt(&inner.dir, &sender, &caller.names, &id).await?,
    };
    match previous {
        Some(receipt) if receipt.digest == digest => return Ok(Some((sender, id, receipt))),
        Some(_) => {
            set_aside(format!("from {}: the id {} was used before for another letter", caller.machine, id.as_str()))?;
            return Ok(None);
        }
        None => {}
    }
    let from = caller.machine.clone();
    caller.received = Some(reference.clone());
    let handled = match inner.node.letter(caller, purpose, id.clone(), letter).await {
        Ok(handled) => handled,
        Err(error) if give_up && lasting(&error) => Err(format!("{error}, every time it was taken")),
        Err(error) => return Err(error),
    };
    if let Err(reason) = handled {
        set_aside(format!("from {from}: {reason}"))?;
    }
    let receipt = Receipt { digest, at: Timestamp::now() };
    recent.insert((sender, id.clone()), receipt);
    Ok(Some((sender, id, receipt)))
}
