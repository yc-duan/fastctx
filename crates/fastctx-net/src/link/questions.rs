//! Hub questions own their partial answers. Removing a question releases its
//! pieces under the same lock that receives them, so late parts cannot recreate
//! an abandoned assembly.

use std::collections::HashMap;
use std::sync::Mutex;

use tokio::sync::oneshot;

use crate::reassembly::Buffer;
use crate::wire::{MAX_CONTROL_FRAME, PARTS_AT_MOST, Told};

const IN_PARTS: usize = 8;
const MAX_PARTS: u32 = 4096;

type Answer = Result<Told, String>;

#[derive(Default)]
pub(super) struct Questions {
    pending: Mutex<HashMap<u64, Pending>>,
}

struct Pending {
    generation: u64,
    answer: oneshot::Sender<Answer>,
    parts: Option<Parts>,
}

struct Parts {
    total: u32,
    came: u32,
    text: Buffer,
}

impl Questions {
    pub(super) fn insert(&self, id: u64, generation: u64, answer: oneshot::Sender<Answer>) {
        self.pending.lock().expect("never poisoned").insert(id, Pending { generation, answer, parts: None });
    }

    pub(super) fn remove(&self, id: u64) {
        let mut pending = self.pending.lock().expect("never poisoned");
        pending.remove(&id);
        if pending.is_empty() {
            *pending = HashMap::new();
        }
    }

    pub(super) fn end(&self, generation: u64) {
        let mut pending = self.pending.lock().expect("never poisoned");
        pending.retain(|_, question| question.generation != generation);
        if pending.is_empty() {
            *pending = HashMap::new();
        }
    }

    pub(super) fn told(&self, generation: u64, id: u64, answer: Answer) {
        let mut pending = self.pending.lock().expect("never poisoned");
        if pending.get(&id).is_none_or(|question| question.generation != generation) {
            return;
        }
        let question = pending.remove(&id).expect("present");
        drop(pending);
        drop(question.parts);
        let _ = question.answer.send(answer);
    }

    pub(super) fn part(&self, generation: u64, id: u64, index: u32, total: u32, text: String) {
        let mut pending = self.pending.lock().expect("never poisoned");
        let Some(question) = pending.get(&id).filter(|question| question.generation == generation) else { return };
        let full = question.parts.is_none()
            && pending
                .values()
                .filter(|question| question.generation == generation && question.parts.is_some())
                .count()
                >= IN_PARTS;
        let question = pending.get_mut(&id).expect("present");
        let result = if full {
            Err(format!("the hub sent more than {IN_PARTS} answers in parts at once"))
        } else {
            question.push(index, total, &text)
        };
        if matches!(result, Ok(false)) {
            return;
        }
        let question = pending.remove(&id).expect("present");
        drop(pending);
        // Decode outside the register lock. The buffer keeps its reservation
        // until decoding ends, and no partial answer remains in the register.
        let answer = result.and_then(|_| {
            let parts = question.parts.expect("a completed answer has parts");
            serde_json::from_slice::<Answer>(&parts.text)
                .unwrap_or_else(|error| Err(format!("the hub's answer cannot be read: {error}")))
        });
        let _ = question.answer.send(answer);
    }
}

impl Pending {
    fn push(&mut self, index: u32, total: u32, text: &str) -> Result<bool, String> {
        if total == 0 || total > MAX_PARTS || index >= total || text.is_empty() {
            return Err("the hub's answer has invalid part numbering or an empty part".to_string());
        }
        if self.parts.is_none() {
            if index != 0 {
                return Err("the hub's answer did not start with its first part".to_string());
            }
            // Each part came in one frame, which bounds what all of them hold.
            let most = PARTS_AT_MOST.min(total as usize * MAX_CONTROL_FRAME);
            self.parts = Some(Parts { total, came: 0, text: Buffer::new(most, 0)? });
        }
        let parts = self.parts.as_mut().expect("started");
        if parts.total != total || parts.came != index {
            return Err("the hub's answer came in parts that do not fit together".to_string());
        }
        parts.text.extend(text.as_bytes())?;
        parts.came += 1;
        Ok(parts.came == total)
    }
}
