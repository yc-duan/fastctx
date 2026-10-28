//! Conditional text delivery never creates a turn when the observed work has ended.

use super::server::Unanswered;
use super::{CodexHost, SendError, Sent, ServerUse};
use serde_json::json;
use std::sync::Arc;

impl CodexHost {
    /// Joins the currently observed turn atomically at the host. `None` means no
    /// running turn was available; an RPC refusal also leaves the text unsubmitted.
    /// `client` identifies the message for delivery reconciliation, not RPC replay.
    pub async fn send_to_running(
        self: &Arc<Self>,
        thread: &str,
        text: String,
        client: Option<String>,
    ) -> Result<Option<Sent>, SendError> {
        let (host, thread) = (self.clone(), thread.to_string());
        tokio::spawn(async move { host.steer_inner(&thread, text, client).await })
            .await
            .map_err(|error| SendError::Unknown(format!("the turn steering task stopped: {error}")))?
    }

    async fn steer_inner(
        self: &Arc<Self>,
        thread: &str,
        text: String,
        client: Option<String>,
    ) -> Result<Option<Sent>, SendError> {
        let (server, turn) = {
            let mut state = self.lock();
            let Some(turn) = state.threads.get(thread).and_then(|entry| entry.turn.clone()) else { return Ok(None) };
            let Some((server, _)) = &state.server else { return Ok(None) };
            let server = server.clone();
            state.server_users += 1;
            (ServerUse::new(server, self.clone()), turn)
        };
        let mut params = json!({"threadId":thread, "expectedTurnId":turn,
            "input":[{"type":"text","text":text,"text_elements":[]}]});
        if let Some(client) = client {
            params["clientUserMessageId"] = json!(client);
        }
        match server.ask("turn/steer", params).await {
            Ok(answer) if answer["turnId"].as_str() == Some(turn.as_str()) => Ok(Some(Sent { turn, joined: true })),
            Ok(_) => Err(SendError::Unknown("turn/steer answered without the expected turn id".into())),
            Err(Unanswered::Refused(why) | Unanswered::Rpc { message: why, .. }) => Err(SendError::NotSent(why)),
            Err(Unanswered::Stopped(why)) => Err(SendError::Unknown(why)),
        }
    }
}
