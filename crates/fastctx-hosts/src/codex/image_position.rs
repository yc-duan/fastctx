//! Public item positions close live-listener gaps before pictures are exposed.

use std::collections::HashSet;
use std::sync::Arc;

use fastctx_types::Address;
use fastctx_types::session::{Happened, SessionEvent, images};
use serde_json::{Value, json};

use super::{CodexHost, history, server::Unanswered, user_images};

impl CodexHost {
    /// Resolve a live user picture's native position before World messages
    /// are restored. Canonical IDs address the public per-turn item list;
    /// legacy snapshots supply their own replay identities instead.
    pub async fn locate_user_images(
        self: &Arc<Self>,
        thread: &str,
        owner: &Address,
        mut event: SessionEvent,
    ) -> Result<Vec<SessionEvent>, String> {
        let Happened::Heard { turn: Some(turn), message: Some(message), .. } = &mut event.happened else {
            return Ok(vec![event]);
        };
        if message.detail["type"] != "userMessage"
            || !message.content.iter().any(|block| matches!(block["type"].as_str(), Some("image" | "localImage")))
        {
            return Ok(vec![event]);
        }
        // A kept local picture already has its upload address; no native
        // coordinates are exposed until an unrepresented picture exists.
        images::codex(message, owner, turn, 0, false);
        if message.images.is_empty() {
            return Ok(vec![event]);
        }
        let server = self.server().await?;
        let mut params = json!({"threadId":thread,"turnId":turn,"sortDirection":"asc","limit":100});
        let mut ids = HashSet::new();
        loop {
            let page = match server.ask("thread/items/list", params.clone()).await {
                Ok(page) => page,
                Err(Unanswered::Rpc { code: -32601, message: reason, data })
                    if reason == "thread/items/list is not supported yet" && data.is_null() =>
                {
                    let known =
                        self.lock().threads.get(thread).and_then(|entry| entry.submissions.known(turn, &message.id));
                    let answer = server.request("thread/read", json!({"threadId":thread,"includeTurns":true})).await?;
                    let snapshot:Vec<_>=history::project(&answer,None,Some(owner)).into_iter()
                        .filter(|event|matches!(&event.happened,Happened::Heard {turn:Some(t),message:Some(message),..} if t==turn && message.detail["type"]=="userMessage")).collect();
                    let matching: Vec<_> = snapshot
                        .iter()
                        .filter_map(|event| match &event.happened {
                            Happened::Heard { message: Some(other), .. }
                                if user_images::same_content(other, message) =>
                            {
                                let fastctx_types::attachments::AttachmentSource::HostImage {
                                    host_image: fastctx_types::attachments::HostImage::Codex { message: ordinal, .. },
                                    ..
                                } = &other.images.first()?.source
                                else {
                                    return None;
                                };
                                Some((*ordinal, event))
                            }
                            _ => None,
                        })
                        .collect();
                    let target = known
                        .and_then(|index| matching.iter().find(|(candidate, _)| *candidate == index))
                        .or_else(|| (matching.len() == 1).then(|| &matching[0]));
                    if let Some((ordinal, found)) = target {
                        if let Happened::Heard { message: Some(found), .. } = &found.happened {
                            message.id = found.id.clone();
                        }
                        images::codex(message, owner, turn, *ordinal, true);
                        return Ok(vec![event]);
                    }
                    if matching.is_empty() {
                        return Err("the public Codex history contains no matching live picture submission".into());
                    }
                    // Legacy IDs are rebuilt. Identical inputs after a
                    // listener gap cannot identify one row; replay the
                    // authoritative pictures with their individual positions.
                    return Ok(snapshot.into_iter().filter(|event|matches!(&event.happened,Happened::Heard {message:Some(message),..} if !message.images.is_empty())).collect());
                }
                Err(error) => return Err(error.to_string()),
            };
            let entries = page["data"].as_array().ok_or("Codex supplied no per-turn item list")?;
            for entry in entries {
                let item = &entry["item"];
                if item["type"] != "userMessage" {
                    continue;
                }
                let id = item["id"].as_str().ok_or("Codex supplied a user submission without its identity")?;
                if !ids.insert(id.to_owned()) {
                    continue;
                }
                if id == message.id {
                    let ordinal = u32::try_from(ids.len() - 1).map_err(|error| error.to_string())?;
                    images::codex(message, owner, turn, ordinal, false);
                    return Ok(vec![event]);
                }
            }
            match page["nextCursor"].as_str() {
                Some(cursor) => params["cursor"] = Value::String(cursor.into()),
                None => {
                    return Err(
                        "the public Codex item list contains no position for the live picture submission".into()
                    );
                }
            }
        }
    }
}
