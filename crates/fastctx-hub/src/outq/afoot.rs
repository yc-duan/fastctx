//! Queued status owns cursors into shared snapshots, never a full encoded copy.

use fastctx_net::afoot::Frames;
use fastctx_net::wire::Down;
use fastctx_types::MachineName;
use fastctx_types::town::{AfootSeen, ResidentNow, RunningJob};
use std::sync::Arc;

pub(super) const LIMIT: usize = 32 * 1024 * 1024;
const HEADER: usize = 1024;

pub(super) struct Pending {
    machine: MachineName,
    busy: Option<u32>,
    frames: Frames<ResidentNow, RunningJob>,
    pub(super) initial: bool,
}

impl Pending {
    pub(super) fn new(
        machine: MachineName,
        before: Option<Arc<AfootSeen>>,
        after: Arc<AfootSeen>,
        busy: Option<u32>,
        bytes: &mut usize,
    ) -> Result<Self, &'static str> {
        if *bytes > LIMIT - HEADER {
            return Err("too many pending status snapshots");
        }
        *bytes += HEADER;
        Ok(Self { machine, busy, frames: Frames::new(before, after), initial: false })
    }

    pub(super) fn replace(&mut self, after: Arc<AfootSeen>, busy: Option<u32>) {
        self.frames.replace(after);
        self.busy = busy;
    }

    pub(super) fn take(&mut self, bytes: &mut usize) -> Result<(Down, bool), String> {
        let change = self.frames.next().expect("an active snapshot has a frame")?;
        let done = self.frames.done();
        if done {
            *bytes -= HEADER;
        }
        Ok((Down::Afoot { machine: self.machine.clone(), busy: self.busy, change: Box::new(change) }, done))
    }
}
