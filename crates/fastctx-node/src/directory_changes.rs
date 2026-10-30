//! Directory effects exclude presentation changes and coalesce full ledger refreshes.

use std::sync::{Arc, Mutex};

use fastctx_net::directory::View;
use fastctx_net::link::{Link, Status};
use tokio::sync::Notify;

use crate::Node;

#[derive(Default)]
pub(crate) struct Changes {
    state: Mutex<State>,
    wake: Notify,
}

#[derive(Default)]
struct State {
    view: Option<Arc<View>>,
    pending: bool,
    running: bool,
}

impl Changes {
    /// Records the latest view, including when only its displayed fields changed.
    pub(crate) fn changed(&self, view: Arc<View>) -> bool {
        let mut state = self.state.lock().expect("never poisoned");
        let changed = state.view.as_ref().is_none_or(|previous| view.records_changed(previous));
        state.view = Some(view);
        changed
    }

    pub(crate) fn reconcile(self: &Arc<Self>, node: &Arc<Node>, link: Arc<Link>) {
        cleanup(node, &link);
        {
            let mut state = self.state.lock().expect("never poisoned");
            state.pending = true;
            if state.running {
                self.wake.notify_one();
                return;
            }
            state.running = true;
        }
        let (changes, node) = (self.clone(), node.clone());
        tokio::spawn(async move {
            let mut again = crate::again::Again::default();
            let mut previous = None;
            loop {
                let current = node.link_of(link.hub()).is_some_and(|current| Arc::ptr_eq(&current, &link));
                {
                    let mut state = changes.state.lock().expect("never poisoned");
                    if !state.pending || !current {
                        state.running = false;
                        return;
                    }
                    state.pending = false;
                }
                match crate::ledger::sync(&node, link.hub(), None).await {
                    Ok(()) => {
                        again = crate::again::Again::default();
                        previous = None;
                        cleanup(&node, &link);
                        node.shown_changed();
                    }
                    Err(error) => {
                        if previous.as_ref() != Some(&error) {
                            node.log.line(format_args!("taking the projects of this computer's accounts: {error}"));
                            previous = Some(error);
                        }
                        if !matches!(link.status(), Status::Online { .. }) {
                            changes.state.lock().expect("never poisoned").running = false;
                            return;
                        }
                        changes.state.lock().expect("never poisoned").pending = true;
                        tokio::select! {
                            () = tokio::time::sleep(again.next()) => {},
                            () = changes.wake.notified() => {},
                        }
                    }
                }
            }
        });
    }
}

/// A new revocation never waits for an older read or another conversation's
/// management gate. Ordinary ledger notifications retain their own worker too:
/// a failed full reconciliation cannot suppress a healthy project's update.
pub(crate) fn cleanup(node: &Arc<Node>, link: &Arc<Link>) {
    let (node, link) = (node.clone(), link.clone());
    tokio::spawn(async move {
        if node.link_of(link.hub()).is_some_and(|current| Arc::ptr_eq(&current, &link)) {
            crate::ledger::letgo::let_go(&node, &link).await;
        }
    });
}
