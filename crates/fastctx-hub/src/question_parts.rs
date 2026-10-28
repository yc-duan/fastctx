//! Upload assemblies belong to one authenticated connection and expire with it.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use fastctx_net::reassembly::{Buffer, Reservation};
use fastctx_net::wire::{self, Question};

const WITHIN: Duration = Duration::from_secs(60);
const SIMULTANEOUS: usize = 8;

#[derive(Default)]
pub(crate) struct Uploads {
    pending: HashMap<u64, Partial>,
}

struct Partial {
    began: Instant,
    total: u32,
    next: u32,
    text: Buffer,
}

pub(crate) struct Complete {
    pub question: Question,
    pub reservation: Reservation,
}

impl Uploads {
    pub fn cancel(&mut self, id: u64) {
        self.pending.remove(&id);
    }

    pub fn expired(&mut self) -> Vec<u64> {
        let expired: Vec<_> =
            self.pending.iter().filter(|(_, partial)| partial.began.elapsed() >= WITHIN).map(|(id, _)| *id).collect();
        for id in &expired {
            self.pending.remove(id);
        }
        expired
    }

    pub fn push(&mut self, id: u64, index: u32, total: u32, text: &str) -> Result<Option<Complete>, String> {
        let result = self.accept(id, index, total, text);
        if result.is_err() {
            self.cancel(id);
        }
        result
    }

    fn accept(&mut self, id: u64, index: u32, total: u32, text: &str) -> Result<Option<Complete>, String> {
        if total == 0 || total > 4096 || index >= total || text.is_empty() {
            return Err("invalid ledger upload part numbering or empty part".into());
        }
        if !self.pending.contains_key(&id) {
            if index != 0 {
                return Err("the ledger upload did not start with part zero".into());
            }
            if self.pending.len() >= SIMULTANEOUS {
                return Err("too many ledger uploads on this connection".into());
            }
            let limit = wire::PARTS_AT_MOST.min(total as usize * wire::MAX_CONTROL_FRAME);
            self.pending.insert(id, Partial { began: Instant::now(), total, next: 0, text: Buffer::new(limit, 0)? });
        }
        let partial = self.pending.get_mut(&id).expect("inserted above");
        if partial.total != total || partial.next != index {
            return Err("ledger upload parts do not fit together".into());
        }
        if partial.began.elapsed() >= WITHIN {
            return Err("the ledger upload exceeded 60 seconds".into());
        }
        partial.text.extend(text.as_bytes())?;
        partial.next += 1;
        if partial.next != total {
            return Ok(None);
        }
        let partial = self.pending.remove(&id).expect("complete upload");
        let question: Question =
            serde_json::from_slice(&partial.text).map_err(|error| format!("invalid ledger upload: {error}"))?;
        if !matches!(question, Question::LedgerWrite { .. } | Question::MemoryWrite { .. }) {
            return Err("only ledger appends can be uploaded in parts".into());
        }
        Ok(Some(Complete { question, reservation: partial.text.into_reservation() }))
    }
}
