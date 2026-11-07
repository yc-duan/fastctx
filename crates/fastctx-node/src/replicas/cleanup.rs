//! Coalesced withdrawal retries outlive the membership that required them.

use crate::Node;
use std::sync::{Arc, Mutex};

#[derive(Default)]
pub(crate) struct Cleanup {
    pending: Mutex<Pending>,
    wake: tokio::sync::Notify,
}

#[derive(Default)]
struct Pending {
    started: bool,
    asked: bool,
    running: bool,
}

pub(crate) fn start(node: &Arc<Node>) {
    node.replicas.cleanup.pending.lock().expect("never poisoned").started = true;
    schedule(node);
}

pub(crate) fn started(node: &Node) -> bool {
    node.replicas.cleanup.pending.lock().expect("never poisoned").started
}

pub(crate) fn schedule(node: &Arc<Node>) {
    {
        let mut pending = node.replicas.cleanup.pending.lock().expect("never poisoned");
        pending.asked = true;
        if !pending.started {
            return;
        }
        if pending.running {
            node.replicas.cleanup.wake.notify_one();
            return;
        }
        pending.running = true;
    }
    let node = node.clone();
    fastctx_platform::task::spawn(async move {
        let mut again = crate::again::Again::default();
        let mut previous = None;
        loop {
            {
                let mut pending = node.replicas.cleanup.pending.lock().expect("never poisoned");
                if !std::mem::take(&mut pending.asked) {
                    pending.running = false;
                    return;
                }
            }
            let working = node.clone();
            match fastctx_platform::task::blocking(move || super::publication::prune_now(&working)).await {
                Ok(()) => {
                    again.reset();
                    previous = None;
                }
                Err(error) => {
                    if previous.as_ref() != Some(&error) {
                        node.log.line(format_args!("withdrawing document replicas: {error}"));
                        previous = Some(error);
                    }
                    node.replicas.cleanup.pending.lock().expect("never poisoned").asked = true;
                    tokio::select! {
                        () = tokio::time::sleep(again.next()) => {},
                        () = node.replicas.cleanup.wake.notified() => {},
                    }
                }
            }
        }
    });
}
