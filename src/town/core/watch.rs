//! Watching residents live: their history first, then each event as it happens.

use super::{Route, Town};
use crate::town::talk::TalkEvent;
use tokio::sync::mpsc;

impl Town {
    /// Starts watching a resident on this machine: its history so far, then its events as they
    /// happen. The watcher is registered before the history is read so nothing falls between;
    /// what appears in both is for the caller to drop (`talk::HistoryKeys`).
    pub(crate) async fn watch(&self, id: &str) -> Result<(Vec<TalkEvent>, mpsc::UnboundedReceiver<TalkEvent>), String> {
        let (route, receiver) = {
            let mut state = self.state.lock();
            let route = state
                .residents
                .get(id)
                .map(|live| live.route.clone())
                .ok_or_else(|| format!("No resident named {id} lives on {}.", self.config.machine))?;
            let (sender, receiver) = mpsc::unbounded_channel();
            state.watchers.entry(id.to_string()).or_default().push(sender);
            (route, receiver)
        };
        let history = match route {
            Route::CodexThread(thread) => {
                let history = self.codex.history(&thread).await?;
                crate::town::talk::codex_history(&history, self.codex.running_turn(&thread).as_deref())
            }
            Route::ClaudeMod(_) | Route::Piggyback => Vec::new(),
        };
        Ok((history, receiver))
    }

    pub(super) fn publish_thread(&self, thread: &str, event: TalkEvent) {
        let resident = self.state.lock().threads.get(thread).cloned();
        if let Some(resident) = resident {
            self.publish(&resident, event);
        }
    }

    pub(super) fn publish_session(&self, session: &str, event: TalkEvent) {
        let resident = self
            .state
            .lock()
            .residents
            .iter()
            .find(|(_, live)| matches!(&live.route, Route::ClaudeMod(Some(paired)) if paired == session))
            .map(|(id, _)| id.clone());
        if let Some(resident) = resident {
            self.publish(&resident, event);
        }
    }

    /// Hands an event to everyone watching the resident; watchers that went away are dropped.
    pub(super) fn publish(&self, resident: &str, event: TalkEvent) {
        let mut state = self.state.lock();
        if let Some(watchers) = state.watchers.get_mut(resident) {
            watchers.retain(|watcher| watcher.send(event.clone()).is_ok());
            if watchers.is_empty() {
                state.watchers.remove(resident);
            }
        }
    }
}
