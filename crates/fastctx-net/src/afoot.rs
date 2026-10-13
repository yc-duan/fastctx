//! What is afoot on a machine goes between it, its hub and the members as
//! changes (`world-network.md`, 近况). A change goes in one control frame
//! when it fits, and as several when it does not, the first of them `anew`
//! when the change is.

use fastctx_types::town::{Afloat, AfootChange};
use serde::Serialize;

/// What a control frame leaves for the change it carries. Around the change
/// are the frame's kind, the machine it is of (a name is at most 64 bytes,
/// which JSON writes as they are) and how many are busy there; a kibibyte
/// holds them many times over.
pub const FRAME_ROOM: usize = crate::wire::MAX_CONTROL_FRAME - 1024;

/// What a change takes before anything it carries: its braces and the keys
/// of its five lists, with their quotes, colons and brackets.
const BARE: usize = 128;

/// `change` as changes that each fit in `room` bytes, in the order they are
/// to be told, and the ids of what does not fit in a frame of its own: they
/// are left out, for the caller to say so.
pub fn frames<R, J>(change: AfootChange<R, J>, room: usize) -> (Vec<AfootChange<R, J>>, Vec<String>)
where
    R: Afloat + Serialize,
    J: Afloat + Serialize,
    R::Id: Serialize,
    J::Id: Serialize,
{
    let AfootChange { anew, residents, residents_gone, jobs, jobs_gone } = change;
    let mut split = Split { room, frames: Vec::new(), now: AfootChange { anew, ..AfootChange::default() }, used: BARE };
    let mut too_large = Vec::new();
    for id in residents_gone {
        if split.make_room(size(&id)) {
            split.now.residents_gone.push(id);
        } else {
            too_large.push(label(&id));
        }
    }
    for id in jobs_gone {
        if split.make_room(size(&id)) {
            split.now.jobs_gone.push(id);
        } else {
            too_large.push(label(&id));
        }
    }
    for resident in residents {
        if split.make_room(size(&resident)) {
            split.now.residents.push(resident);
        } else {
            too_large.push(label(&resident.id()));
        }
    }
    for job in jobs {
        if split.make_room(size(&job)) {
            split.now.jobs.push(job);
        } else {
            too_large.push(label(&job.id()));
        }
    }
    let Split { mut frames, now, .. } = split;
    if !now.is_empty() {
        frames.push(now);
    }
    (frames, too_large)
}

struct Split<R, J> {
    room: usize,
    frames: Vec<AfootChange<R, J>>,
    now: AfootChange<R, J>,
    used: usize,
}

impl<R, J> Split<R, J> {
    /// Makes room for one more thing of `size` bytes, starting a new frame
    /// when this one is full; false when it fits in no frame.
    fn make_room(&mut self, size: Option<usize>) -> bool {
        let Some(size) = size.filter(|size| BARE + size <= self.room) else { return false };
        if self.used + size > self.room {
            self.frames.push(std::mem::take(&mut self.now));
            self.used = BARE;
        }
        self.used += size;
        true
    }
}

/// How many bytes `value` takes in a list: its JSON and a comma.
fn size(value: &impl Serialize) -> Option<usize> {
    serde_json::to_vec(value).ok().map(|bytes| bytes.len() + 1)
}

fn label(id: &impl Serialize) -> String {
    serde_json::to_string(id).unwrap_or_default()
}
