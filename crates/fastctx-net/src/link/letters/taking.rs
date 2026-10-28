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

pub(in crate::link) async fn take(inner: Arc<Inner>, epoch: u64, run: Vec<(u64, Envelope)>) -> io::Result<u64> {
    let held = inner.taking.lock_owned().await;
    tokio::spawn(async move {
        let _doing = fastctx_platform::doing::doing(format!("persisting letters from {}", inner.hub));
        held.adopt();
        let _held = held;
        let taken = inner.letters.lock().expect("never poisoned").file.taken;
        let mut pending = Vec::new();
        let mut recent = HashMap::new();
        let mut last = None;
        for (sequence, message) in run {
            if !taken.is_some_and(|(known, taken)| known == epoch && sequence <= taken) {
                let reference = Reference { authority: inner.hub.as_str().to_owned(), epoch, sequence };
                match one(&inner, &message, &mut recent, &reference).await {
                    Ok(Some((from, id, receipt))) => pending.push((from, id, receipt, reference)),
                    Ok(None) => {}
                    Err(error) => {
                        // Earlier business handoffs in this window still need
                        // their receipts, even when a later handoff must retry.
                        receipts::keep_batch(&inner.dir, pending).await?;
                        return Err(error);
                    }
                }
            }
            last = Some(sequence);
        }
        receipts::keep_batch(&inner.dir, pending).await?;
        let sequence = last.expect("receive windows are nonempty");
        let mut letters = inner.letters.lock().expect("never poisoned");
        if !letters.file.taken.is_some_and(|(known, taken)| known == epoch && sequence <= taken) {
            let mut next = letters.file;
            next.taken = Some((epoch, sequence));
            super::super::state::write(&inner.dir.letters(), &next)?;
            letters.file = next;
        }
        Ok(sequence)
    })
    .await
    .map_err(io::Error::other)?
}

/// Admission and business errors are set aside durably. Local storage failures
/// leave the window unacknowledged so the hub delivers it again.
async fn one(
    inner: &Inner,
    message: &Envelope,
    recent: &mut HashMap<(PublicKey, LetterId), Receipt>,
    reference: &Reference,
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
    if let Err(reason) = inner.node.letter(caller, purpose, id.clone(), letter).await? {
        set_aside(format!("from {from}: {reason}"))?;
    }
    let receipt = Receipt { digest, at: Timestamp::now() };
    recent.insert((sender, id.clone()), receipt);
    Ok(Some((sender, id, receipt)))
}
