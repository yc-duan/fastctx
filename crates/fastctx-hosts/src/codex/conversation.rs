//! A conversation owns its process, while retaining its login and session store.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use fastctx_types::ResidentId;
use tokio::sync::mpsc;

use super::{CodexHost, HostEvent, ThreadSpec, assisted::Assisted};

impl CodexHost {
    /// Creates a lazy, independent process for one conversation. Its session
    /// directory and login authority remain those of this host, including
    /// conversations created before process isolation was introduced.
    pub fn for_conversation(
        &self,
        owner: ResidentId,
        workdir: PathBuf,
    ) -> (Arc<Self>, mpsc::UnboundedReceiver<HostEvent>) {
        let (mut host, events) =
            Self::new(self.codex_home.clone(), self.program_home.clone(), workdir, self.env.clone(), self.log.clone());
        let fresh = Arc::get_mut(&mut host).expect("new host has one owner");
        fresh.assisted = self
            .assisted
            .as_ref()
            .map(|assisted| Assisted { owner: assisted.owner.clone(), home: assisted.home.clone() });
        fresh.conversation = Some(owner);
        let weak = Arc::downgrade(&host);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(60));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                interval.tick().await;
                let Some(host) = weak.upgrade() else { return };
                if let Err(error) = host.retire_unloaded().await {
                    (host.log)(format!("retiring an unloaded Codex process: {error}"));
                }
            }
        });
        (host, events)
    }

    pub(super) fn check_conversation(&self, spec: &ThreadSpec) -> Result<(), String> {
        if self.conversation.as_ref().is_some_and(|owner| owner != &spec.owner.resident) {
            return Err("this Codex process belongs to another conversation".into());
        }
        Ok(())
    }

    /// Ends only the process owned by this conversation, and only while the
    /// specified turn is still current. Does not start a missing process.
    /// The caller must first persist its reason and prevent new submissions.
    pub async fn terminate_turn(&self, thread: &str, turn: &str) -> Result<bool, String> {
        let owner = self.conversation.as_ref().ok_or("this Codex process is shared")?;
        let server = {
            let _generation = self.generation.lock().expect("never poisoned");
            let mut state = self.lock();
            let Some(entry) = state.threads.get(thread) else { return Ok(false) };
            if entry.spec.as_ref().is_none_or(|spec| &spec.owner.resident != owner) {
                return Err("this Codex thread does not own this process".into());
            }
            if entry.turn.as_deref() != Some(turn) {
                return Ok(false);
            }
            let Some((server, _)) = &state.server else { return Ok(false) };
            let server = server.clone();
            server.kill();
            server
        };
        tokio::time::timeout(Duration::from_secs(10), server.wait())
            .await
            .map_err(|_| "the Codex process has not confirmed its exit within 10 s")?;
        Ok(true)
    }

    async fn retire_unloaded(&self) -> Result<(), String> {
        let _starting = self.starting.lock().await;
        let server = {
            let _generation = self.generation.lock().expect("never poisoned");
            let state = self.lock();
            if state.server_users != 0
                || !state.title_waiters.is_empty()
                || state.threads.values().any(|thread| {
                    thread.loaded
                        || thread.submitted
                        || thread.turn.is_some()
                        || !thread.started.is_empty()
                        || !thread.unsettled.is_empty()
                        || !thread.requests.is_empty()
                })
            {
                return Ok(());
            }
            let Some((server, _)) = &state.server else { return Ok(()) };
            let server = server.clone();
            server.kill();
            server
        };
        tokio::time::timeout(Duration::from_secs(10), server.wait())
            .await
            .map_err(|_| "the unloaded Codex process has not confirmed its exit within 10 s")?;
        Ok(())
    }
}
