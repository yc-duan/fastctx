//! Display projection of context sent with a native input. Only a durable
//! handover identity and a byte-exact digest authorize hiding the first block.
//! Forks inherit these small receipts, never another copy of account bodies.
use fastctx_types::{
    NoticeId,
    layout::ConversationDir,
    session::{Happened, SessionEvent},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, io};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Stamp {
    digest: String,
    bytes: usize,
}

pub(crate) fn stamp(notes: &[String]) -> Option<Stamp> {
    if notes.is_empty() {
        return None;
    }
    let text = notes.join("\n\n");
    Some(Stamp { digest: format!("{:x}", Sha256::digest(text.as_bytes())), bytes: text.len() })
}

pub(crate) struct Projection<'a> {
    directory: &'a ConversationDir,
    inherited: BTreeMap<String, Stamp>,
}
impl<'a> Projection<'a> {
    pub(crate) fn open(directory: &'a ConversationDir) -> io::Result<Self> {
        let inherited = match std::fs::read(directory.books().join("context-inputs.json")) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(io::Error::other)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => BTreeMap::new(),
            Err(error) => return Err(error),
        };
        Ok(Self { directory, inherited })
    }
    fn get(&self, client: &str) -> io::Result<Option<Stamp>> {
        if let Some(stamp) = self.inherited.get(client) {
            return Ok(Some(stamp.clone()));
        }
        let Some(id) = client.strip_prefix(fastctx_hosts::codex::WORLD_CLIENT) else { return Ok(None) };
        let Ok(id) = id.parse::<NoticeId>() else { return Ok(None) };
        match std::fs::read(crate::delivery::handover_path(self.directory, &id)) {
            Ok(bytes) => {
                let handover: crate::delivery::Handover = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
                Ok(handover.context_input)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }
    pub(crate) fn restore(&self, event: &mut SessionEvent) -> io::Result<()> {
        let Happened::Heard { text, message: Some(message), .. } = &mut event.happened else { return Ok(()) };
        let Some(client) = &message.client_id else { return Ok(()) };
        let Some(stamp) = self.get(client)? else { return Ok(()) };
        let Some(first) = message.content.first().and_then(|block| block["text"].as_str()) else { return Ok(()) };
        if first.len() != stamp.bytes || format!("{:x}", Sha256::digest(first.as_bytes())) != stamp.digest {
            return Ok(());
        }
        // Preserve native content and detail for inspection; only the shown
        // words exclude the independently delivered context block.
        *text = message
            .content
            .iter()
            .skip(1)
            .filter(|block| block["type"] == "text")
            .filter_map(|block| block["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n");
        Ok(())
    }
}

pub(crate) fn inherit(parent: &ConversationDir, child: &ConversationDir, events: &[SessionEvent]) -> io::Result<()> {
    let source = Projection::open(parent)?;
    let mut inherited = BTreeMap::new();
    for event in events {
        if let Happened::Heard { message: Some(message), .. } = &event.happened
            && let Some(client) = &message.client_id
            && let Some(stamp) = source.get(client)?
        {
            inherited.insert(client.clone(), stamp);
        }
    }
    if !inherited.is_empty() {
        fastctx_platform::ensure_private_dir(&child.books())?;
        fastctx_platform::publish(
            &child.books().join("context-inputs.json"),
            &serde_json::to_vec(&inherited).map_err(io::Error::other)?,
            fastctx_platform::Permissions::Private,
        )?;
    }
    Ok(())
}
