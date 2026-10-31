//! One projection worker owns every reader baseline. Changes are coalesced
//! before computation; no shared lock covers a projection or its encoding.

use crate::Hub;
use fastctx_types::MachineName;
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::Notify;

#[derive(Default)]
struct Pending {
    all: bool,
    machines: HashSet<MachineName>,
    came: HashSet<MachineName>,
}
#[derive(Default)]
pub(super) struct Publishing {
    pending: Mutex<Pending>,
    changed: Arc<Notify>,
}
impl Drop for Publishing {
    fn drop(&mut self) {
        self.changed.notify_one();
    }
}
impl Publishing {
    pub(super) fn request(&self, machine: &MachineName) {
        self.pending.lock().expect("activity changes").machines.insert(machine.clone());
        self.changed.notify_one();
    }
    pub(super) fn came(&self, machine: &MachineName) {
        self.pending.lock().expect("activity changes").came.insert(machine.clone());
        self.changed.notify_one();
    }
    pub(super) fn all(&self) {
        self.pending.lock().expect("activity changes").all = true;
        self.changed.notify_one();
    }
}

pub(crate) fn start(hub: &Arc<Hub>) {
    let publishing = hub.afoots.publishing.clone();
    let changed = publishing.changed.clone();
    let state = Arc::downgrade(&publishing);
    let weak = Arc::downgrade(hub);
    tokio::spawn(async move {
        let mut shown = HashMap::new();
        loop {
            changed.notified().await;
            tokio::time::sleep(Duration::from_millis(20)).await;
            let Some(publishing) = state.upgrade() else { return };
            let pending = std::mem::take(&mut *publishing.pending.lock().expect("activity changes"));
            let Some(hub) = weak.upgrade() else { return };
            let log = hub.log.clone();
            match tokio::task::spawn_blocking(move || {
                if pending.all {
                    super::tell(&hub, &mut shown, super::Scope::All);
                } else {
                    if !pending.came.is_empty() {
                        super::tell(&hub, &mut shown, super::Scope::Come(&pending.came));
                    }
                    if !pending.machines.is_empty() {
                        super::tell(&hub, &mut shown, super::Scope::Machines(&pending.machines));
                    }
                }
                shown
            })
            .await
            {
                Ok(next) => shown = next,
                Err(error) => {
                    log.line(format_args!("publishing activity failed: {error}; rebuilding reader baselines"));
                    shown = HashMap::new();
                    publishing.all();
                }
            }
        }
    });
}
