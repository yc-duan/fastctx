//! What of a session's events an interface draws (`world-gui.md`, 对话:
//! only what the interface draws goes over the wire). The host's raw record
//! of each item stays on the machine the session lives on; words keep their
//! text and the pictures that came with them; a tool keeps what it was asked
//! and the one copy of its output the model got; images become references to
//! where they are kept, named by their bytes.

use std::io::Read as _;

use fastctx_types::Address;
use fastctx_types::attachments::{AttachmentSource, HostImage, ToolImage};
use fastctx_types::session::{
    Content, Does, Fingerprint, Happened, Item, SessionChange, SessionEvent, ToolStatus, UserMessage, What,
};
use serde_json::Value;
use sha2::{Digest as _, Sha256};

/// How the images in a history go to a viewer.
pub enum Pictures<'a> {
    /// As the history holds them: what a machine that serves no image by
    /// reference sent stays as it came.
    Inline,
    /// As references to where `owner`, the session, keeps them.
    Referenced { owner: &'a Address },
}

/// Current requests are supplied by each watch, never restored from disk.
pub fn transient(event: &SessionEvent) -> bool {
    matches!(
        event.happened,
        Happened::Changed { change: SessionChange::Request { .. } | SessionChange::Resolved { .. }, .. }
    )
}

/// Slims `event` to what an interface draws; false when it draws nothing of
/// it.
pub fn slim(event: &mut SessionEvent, pictures: &Pictures) -> bool {
    match &mut event.happened {
        Happened::Heard { message: Some(message), .. } => {
            fingerprints(message);
            // The host's own blocks repeat the words and carry the pictures'
            // bytes; the words are in the event's text and the pictures are
            // named by `images`.
            message.content = Vec::new();
            message.hook_fragments = Vec::new();
            message.presentation = Value::Null;
            message.detail = Value::Null;
            true
        }
        Happened::Began { turn, item } | Happened::Updated { turn, item } | Happened::Finished { turn, item } => {
            slim_item(turn, item, pictures);
            true
        }
        Happened::Steps { steps, .. } => {
            for step in steps {
                step.detail = None;
            }
            true
        }
        // A turn's whole diff repeats the changes its items show.
        Happened::Changed { change: SessionChange::Diff { .. }, .. } => false,
        _ => true,
    }
}

fn slim_item(turn: &str, item: &mut Item, pictures: &Pictures) {
    let Item { id, detail, what, .. } = item;
    // Of the raw record an interface reads only the questions Codex marks a
    // message with when its agent asks without waiting.
    *detail = match (&*what, detail.take()) {
        (What::Message { .. }, Some(detail)) => {
            detail.get("questions").filter(|questions| questions.is_array()).map(|questions| {
                let mut kept = serde_json::Map::new();
                kept.insert("questions".into(), questions.clone());
                Value::Object(kept)
            })
        }
        _ => None,
    };
    let What::Tool(tool) = what else { return };
    // A command is shown by its command line and a change by its diffs,
    // which its input repeats.
    if matches!(tool.does, Does::Command { .. } | Does::Edit { .. }) {
        tool.input = Value::Null;
    }
    // A World run that ended carries the model's copy of its output as
    // facts; the host's text of it is the same output again.
    if tool.run.is_some() && matches!(tool.does, Does::Command { .. }) && tool.status != ToolStatus::Running {
        tool.output.retain(|content| !matches!(content, Content::Text { .. }));
    }
    let Pictures::Referenced { owner } = pictures else { return };
    let mut ordinal = 0u32;
    for content in &mut tool.output {
        let Content::Image { mime, base64 } = content else { continue };
        let image = ordinal;
        ordinal += 1;
        // Bytes that are not base64 are no image to keep; they stay as the
        // host recorded them.
        let Some(print) = fingerprint(base64) else { continue };
        *content = Content::ImageRef {
            mime: std::mem::take(mime),
            content: print,
            source: Box::new(AttachmentSource::ToolImage {
                owner: (*owner).clone(),
                tool_image: ToolImage { turn: (!turn.is_empty()).then(|| turn.to_string()), item: id.clone(), image },
            }),
        };
    }
}

/// The bytes a base64 text holds, by size and SHA-256; `None` when it is
/// not base64.
pub fn fingerprint(base64: &str) -> Option<Fingerprint> {
    let mut decoded = base64::read::DecoderReader::new(base64.as_bytes(), &base64::engine::general_purpose::STANDARD);
    let mut digest = Sha256::new();
    let mut size = 0u64;
    let mut block = [0u8; 16384];
    loop {
        match decoded.read(&mut block) {
            Ok(0) => break,
            Ok(read) => {
                size += read as u64;
                digest.update(&block[..read]);
            }
            Err(_) => return None,
        }
    }
    Some(Fingerprint { size, sha256: format!("{:x}", digest.finalize()) })
}

/// Names each picture of `message` by its bytes, where its host's blocks
/// carry them and say unambiguously which block it is.
fn fingerprints(message: &mut UserMessage) {
    let blocks: Vec<&Value> =
        message.content.iter().filter(|block| matches!(block["type"].as_str(), Some("image" | "localImage"))).collect();
    // Codex numbers its pictures one way in its current history and another
    // in older records whenever a message mixes inline and local pictures;
    // with only inline ones the numbers agree.
    let mixed = blocks.iter().any(|block| block["type"] == "localImage");
    for image in &mut message.images {
        let AttachmentSource::HostImage { host_image, .. } = &image.source else { continue };
        let data = match host_image {
            HostImage::Claude { image: ordinal, .. } => {
                blocks.get(*ordinal as usize).and_then(|block| block["source"]["data"].as_str())
            }
            HostImage::Codex { image: ordinal, .. } if !mixed => blocks
                .get(*ordinal as usize)
                .and_then(|block| block["url"].as_str())
                .and_then(|url| url.strip_prefix("data:"))
                .and_then(|url| url.split_once(";base64,"))
                .map(|(_, data)| data),
            HostImage::Codex { .. } => None,
        };
        if let Some(content) = data.and_then(fingerprint) {
            image.content = Some(content);
        }
    }
}
