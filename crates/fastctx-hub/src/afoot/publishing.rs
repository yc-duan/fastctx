//! Coalesces activity reports and new readers without occupying connection tasks.

use crate::Hub;
use fastctx_types::MachineName;
use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::Notify;

#[derive(Default)]
pub(super) struct Publishing {
    pending: Mutex<HashSet<MachineName>>,
    changed: Arc<Notify>,
}

impl Drop for Publishing {
    fn drop(&mut self) {
        self.changed.notify_one();
    }
}

impl Publishing {
    pub(super) fn request(&self, machine: Option<&MachineName>) {
        if let Some(machine) = machine {
            self.pending.lock().expect("never poisoned").insert(machine.clone());
        }
        self.changed.notify_one();
    }
}

pub(crate) fn start(hub: &Arc<Hub>) {
    let publishing = hub.afoots().publishing.clone();
    let changed = publishing.changed.clone();
    let state = Arc::downgrade(&publishing);
    let weak = Arc::downgrade(hub);
    tokio::spawn(async move {
        let mut next = tokio::time::Instant::now();
        loop {
            changed.notified().await;
            tokio::time::sleep_until(next).await;
            let Some(hub) = weak.upgrade() else { return };
            let Some(publishing) = state.upgrade() else { return };
            let machines = std::mem::take(&mut *publishing.pending.lock().expect("never poisoned"));
            let retry = machines.clone();
            let log = hub.log.clone();
            let result = tokio::task::spawn_blocking(move || {
                super::tell(
                    &hub,
                    if machines.is_empty() { super::Scope::Come } else { super::Scope::Machines(&machines) },
                );
            })
            .await;
            if let Err(error) = result {
                log.line(format_args!("publishing machine activity: {error}"));
                publishing.pending.lock().expect("never poisoned").extend(retry);
                publishing.changed.notify_one();
            }
            next = tokio::time::Instant::now() + Duration::from_millis(20);
        }
    });
}
