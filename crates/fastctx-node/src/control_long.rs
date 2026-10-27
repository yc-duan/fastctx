//! Controls that take long in their host run outside the conversation's
//! action gate (`world-concurrency.md`): a Claude compaction answers only
//! once it is done, a minute or two later. While one runs, words wait in
//! the inbox and other controls in the queue, a stop goes through at once,
//! and as it ends both go on. A Codex compaction needs none of this: its
//! host answers as it starts, and it ends with a turn of its own.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use fastctx_types::ResidentId;
use fastctx_types::session::control::{Control, Controlled, PendingControl};

use crate::Node;

/// Why words wait while a long control runs, as the inbox keeps it.
pub(crate) const WAITING: &str = "the conversation is compacting; the words go in once it is done";

/// The conversations a long control runs in now.
#[derive(Default)]
pub(crate) struct Running(Mutex<HashSet<ResidentId>>);

impl Running {
    /// Whether a long control of `id` is running now.
    pub(crate) fn of(&self, id: &ResidentId) -> bool {
        self.0.lock().expect("never poisoned").contains(id)
    }
}

/// Whether `control` takes long in its host and so runs outside the gate.
pub(crate) fn long(node: &Node, id: &ResidentId, control: &Control) -> bool {
    let compacts = matches!(control, Control::Compact)
        || matches!(control, Control::Command { name, path: None, .. } if name == "compact");
    compacts && node.residents.about(id).is_some_and(|(about, _)| about.host.agent.as_str() == "claude")
}

/// Runs `row`'s control in a task of its own once the caller lets the gate
/// go; it is told in its conversation as it ends, and what waited for it
/// goes on then. The caller hears at once that it began.
pub(crate) fn start(node: &Arc<Node>, id: &ResidentId, row: PendingControl) -> Controlled {
    node.long_controls.0.lock().expect("never poisoned").insert(id.clone());
    let (node, id) = (node.clone(), id.clone());
    tokio::spawn(async move {
        let result = crate::control::execute(&node, &id, row.control.clone()).await;
        if let Err(error) = crate::control_queue::end(&node, &id, row, result).await
            && error.code != Some(fastctx_types::ipc::FailureCode::Told)
        {
            node.log.line(format_args!("telling how {id}'s long control ended: {}", error.message));
        }
        node.long_controls.0.lock().expect("never poisoned").remove(&id);
        crate::control_queue::schedule(&node, &id);
        crate::lifecycle::retry::kick(&node, &id);
    });
    Controlled::Reported { said: None, queued: None }
}
