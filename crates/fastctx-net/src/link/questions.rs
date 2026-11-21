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

/// How long, in time the machine is awake, questions may go unanswered —
/// no answer, no part of one, no progress of one coming — while the hub is
/// heard before the session is taken for stalled.
const STALLED_AFTER: std::time::Duration = std::time::Duration::from_secs(90);
/// How many questions must go unanswered in that time: one long answer
/// alone is no stall.
const STALLED_ASKED: u32 = 3;

type Answer = Result<Told, String>;

#[derive(Default)]
pub(super) struct Questions {
    pending: Mutex<HashMap<u64, Pending>>,
    /// When anything of an answer last came, in time awake, and how many
    /// questions were asked since: whether the session goes on answering
    /// (`stalled`).
    watch: Mutex<Watch>,
}

#[derive(Default)]
struct Watch {
    told: Option<std::time::Duration>,
    asked: u32,
}

/// The time this machine has been awake, where it can be told.
fn awake() -> Option<std::time::Duration> {
    fastctx_platform::awake::Sample::now().ok().map(|sample| sample.awake)
}

struct Pending {
    generation: u64,
    answer: oneshot::Sender<Answer>,
    parts: Option<Parts>,
    progress: Option<tokio::sync::watch::Sender<fastctx_types::update::PackageProgress>>,
}

struct Parts {
    total: u32,
    came: u32,
    text: Buffer,
}

impl Questions {
    pub(super) fn insert(
        &self,
        id: u64,
        generation: u64,
        answer: oneshot::Sender<Answer>,
        progress: Option<tokio::sync::watch::Sender<fastctx_types::update::PackageProgress>>,
    ) {
        self.pending.lock().expect("never poisoned").insert(id, Pending { generation, answer, parts: None, progress });
        let mut watch = self.watch.lock().expect("never poisoned");
        watch.asked = watch.asked.saturating_add(1);
    }

    /// A session begins: its answers are watched from now.
    pub(super) fn watch_anew(&self) {
        *self.watch.lock().expect("never poisoned") = Watch::default();
    }

    /// Something of an answer came on the session: a whole answer, a part of
    /// one, its progress, or a late answer to a question given up on. Slow
    /// as it may be, the hub is answering.
    fn answered(&self) {
        let mut watch = self.watch.lock().expect("never poisoned");
        watch.asked = 0;
        watch.told = awake();
    }

    /// Whether the session begun at `began` (time awake) is stalled: so many
    /// questions asked since the last answer, or since it began, and none
    /// answered for so long. How many, and for how long, if so.
    pub(super) fn stalled(&self, began: std::time::Duration) -> Option<(u32, std::time::Duration)> {
        let watch = self.watch.lock().expect("never poisoned");
        let quiet = awake()?.saturating_sub(watch.told.unwrap_or(began));
        (watch.asked >= STALLED_ASKED && quiet >= STALLED_AFTER).then_some((watch.asked, quiet))
    }

    pub(super) fn progress(&self, generation: u64, id: u64, progress: fastctx_types::update::PackageProgress) {
        self.answered();
        let pending = self.pending.lock().expect("never poisoned");
        if let Some(sender) = pending.get(&id).filter(|p| p.generation == generation).and_then(|p| p.progress.as_ref())
        {
            sender.send_replace(progress);
        }
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
        self.answered();
        let mut pending = self.pending.lock().expect("never poisoned");
        if pending.get(&id).is_none_or(|question| question.generation != generation) {
            return;
        }
        let question = pending.remove(&id).expect("present");
        drop(pending);
        drop(question.parts);
        let _ = question.answer.send(answer);
    }

    /// Keeps one part of an answer; the whole answer, once its last part
    /// came, is handed back to be decoded where blocking is allowed: it can
    /// be many megabytes, and the lines' thread only reads and routes.
    pub(super) fn part(&self, generation: u64, id: u64, index: u32, total: u32, text: String) -> Option<Decode> {
        self.answered();
        let mut pending = self.pending.lock().expect("never poisoned");
        let question = pending.get(&id).filter(|question| question.generation == generation)?;
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
            return None;
        }
        let question = pending.remove(&id).expect("present");
        drop(pending);
        // Decoded outside the register lock. The buffer keeps its reservation
        // until decoding ends, and no partial answer remains in the register.
        Some(Decode {
            answer: question.answer,
            parts: result.map(|_| question.parts.expect("a completed answer has parts")),
        })
    }
}

/// A whole answer that came in parts, to be decoded and handed to whoever
/// asked.
pub(super) struct Decode {
    answer: oneshot::Sender<Answer>,
    parts: Result<Parts, String>,
}

impl Decode {
    pub(super) fn run(self) {
        let answer = self.parts.and_then(|parts| {
            serde_json::from_slice::<Answer>(&parts.text)
                .unwrap_or_else(|error| Err(format!("the hub's answer cannot be read: {error}")))
        });
        let _ = self.answer.send(answer);
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
