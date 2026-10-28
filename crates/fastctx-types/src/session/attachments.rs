//! The World envelope is also a durable contract in host-owned history.
//!
//! What the model reads of a file the user attached is what it is and where
//! a script finds it: `attachments/<id>/<name>` in the conversation's scratch
//! on whichever machine the script runs, where the World brings it first
//! (`world-run.md`, 用户附的文件). It names no machine and no path of one:
//! where a file is kept is the World's to know. The receipts by which the
//! World finds and shows each file stay with the conversation's home, kept
//! by the delivery that brought them (`Receipts`). Envelopes written before
//! carried those receipts in themselves and are still read.

use serde::{Deserialize, Serialize};

use crate::attachments::Attachment;
use crate::{MachineName, NoticeId};

use super::{Happened, SessionEvent, Speaker, UserMessage};

const HEAD: &str = "=== from fastctx (user attachments, v3) ===\n";
/// The head written before 2026-10-29 (UTC), with each file's receipt.
const HEAD_V2: &str = "=== from fastctx (user attachments, v2) ===\n";
/// The head written before 2026-10-09 (UTC); histories still hold it.
const HEAD_V1: &str = "=== from the World (user attachments, v1) ===\n";
const WORDS: &str = "\n=== user's words (verbatim) ===\n";
const EXPLANATION: &str = "The user attached these files; sizes are in bytes. A script finds each at $FASTCTX_SCRATCH/<path> on whichever machine it runs; `fastctx view` on an image shows it to you.";

/// The words of one delivery and the files that came with them, as the
/// World keeps them: each file's receipt. Text after the delimiter is the
/// user's exact input. A delivery id lets native history and live receipts
/// agree.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachedMessage {
    pub delivery: NoticeId,
    pub from: MachineName,
    pub attachments: Vec<Attachment>,
}

/// One attached file as the model is told of it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Told {
    pub id: NoticeId,
    pub name: String,
    pub media_type: String,
    pub size: u64,
    /// Where a script finds it, under the conversation's scratch.
    pub path: String,
}

#[derive(Serialize, Deserialize)]
struct Written {
    delivery: NoticeId,
    from: MachineName,
    files: Vec<Told>,
}

/// The receipts of the files each delivery brought, as the conversation's
/// home keeps them.
pub trait Receipts {
    fn of(&self, delivery: &NoticeId) -> Vec<Attachment>;
}

/// No receipts: for reading envelopes that carry their own.
pub struct NoReceipts;

impl Receipts for NoReceipts {
    fn of(&self, _: &NoticeId) -> Vec<Attachment> {
        Vec::new()
    }
}

/// An envelope as read back from a host's record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Envelope {
    pub delivery: NoticeId,
    pub from: MachineName,
    pub files: Vec<Told>,
    /// The receipts an envelope written before carried in itself.
    carried: Option<Vec<Attachment>>,
}

impl Told {
    pub fn of(attachment: &Attachment) -> Self {
        Self {
            id: attachment.id.clone(),
            name: attachment.file.name.clone(),
            media_type: attachment.file.media_type.clone(),
            size: attachment.file.size,
            path: place(attachment),
        }
    }

    pub fn is_image(&self) -> bool {
        self.media_type.starts_with("image/")
    }
}

/// Where a script finds `attachment` under the conversation's scratch:
/// its own directory, so two files of one name stay apart, under its name
/// made safe as one file name on any system.
pub fn place(attachment: &Attachment) -> String {
    format!("attachments/{}/{}", attachment.id, file_name(&attachment.file.name))
}

/// `name` as one file name on any system: its last part, without the
/// characters some system refuses, never a device name of Windows, never
/// longer than a file system takes, its extension kept.
pub fn file_name(name: &str) -> String {
    const LONGEST: usize = 120;
    let last = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let mut safe: String = last
        .chars()
        .map(|c| if c.is_control() || matches!(c, ':' | '*' | '?' | '"' | '<' | '>' | '|') { '_' } else { c })
        .collect();
    safe = safe.trim().trim_end_matches(['.', ' ']).to_string();
    if safe.is_empty() || safe == "." || safe == ".." {
        safe = "file".to_string();
    }
    let stem = safe.split('.').next().unwrap_or("").to_ascii_uppercase();
    let device = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ((stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.len() == 4
            && stem.as_bytes()[3].is_ascii_digit());
    if device {
        safe.insert(0, '_');
    }
    if safe.len() > LONGEST {
        let extension =
            safe.rfind('.').map(|at| safe[at..].to_string()).filter(|ext| ext.len() <= 16).unwrap_or_default();
        let mut cut = LONGEST - extension.len();
        while !safe.is_char_boundary(cut) {
            cut -= 1;
        }
        safe = format!("{}{extension}", &safe[..cut]);
    }
    safe
}

/// Restores individual World messages from a native host item. Codex can
/// place several text blocks in one item, so each envelope retains its own
/// delivery identity. Native content and detail remain available verbatim.
pub fn restore(event: SessionEvent, receipts: &dyn Receipts) -> Vec<SessionEvent> {
    let Happened::Heard { text, message, turn, .. } = &event.happened else { return vec![event] };
    let mut decoded = Vec::new();
    if let Some(message) = message {
        for block in &message.content {
            if let Some(text) = block.get("text").and_then(serde_json::Value::as_str)
                && let Some((envelope, words)) = Envelope::read(text)
            {
                decoded.push((envelope.attached(receipts), words.to_string()));
            }
        }
    }
    if decoded.is_empty()
        && let Some((envelope, words)) = Envelope::read(text)
    {
        decoded.push((envelope.attached(receipts), words.to_string()));
    }
    if decoded.is_empty() {
        return vec![event];
    }
    let pictures = message.as_ref().map(|message| super::images::delivered(message)).unwrap_or_default();
    decoded
        .into_iter()
        .map(|(metadata, text)| {
            let mut restored = metadata.message(&text);
            restored.images = pictures
                .iter()
                .filter(|(delivery, _)| delivery.as_ref() == Some(&metadata.delivery))
                .map(|(_, image)| image.clone())
                .collect();
            if let Some(native) = message {
                restored.content = native.content.clone();
                restored.detail = native.detail.clone();
                restored.presentation = native.presentation.clone();
            }
            SessionEvent {
                at: event.at,
                happened: Happened::Heard {
                    delivery: Some(metadata.delivery.clone()),
                    turn: turn.clone(),
                    from: Some(metadata.speaker()),
                    text,
                    message: Some(Box::new(restored)),
                    notice: None,
                },
            }
        })
        .collect()
}

impl AttachedMessage {
    pub fn render(&self, text: &str) -> String {
        let written = Written {
            delivery: self.delivery.clone(),
            from: self.from.clone(),
            files: self.attachments.iter().map(Told::of).collect(),
        };
        let metadata = serde_json::to_string(&written).expect("attachment metadata serializes");
        format!("{HEAD}{EXPLANATION}\n{metadata}{WORDS}{text}")
    }

    pub fn message(&self, text: &str) -> UserMessage {
        UserMessage {
            id: self.delivery.to_string(),
            client_id: None,
            content: vec![serde_json::json!({"type": "text", "text": text})],
            hook_fragments: Vec::new(),
            presentation: serde_json::Value::Null,
            detail: serde_json::Value::Null,
            attachments: self.attachments.clone(),
            images: Vec::new(),
        }
    }

    pub fn speaker(&self) -> Speaker {
        Speaker::User { machine: self.from.clone() }
    }
}

impl Envelope {
    /// Recognizes only the complete versioned envelope, never a bare path
    /// or a host's unrelated attachment. Unrecognized input stays verbatim.
    pub fn read(text: &str) -> Option<(Self, &str)> {
        // The explanation is prose; the versioned header and JSON carry
        // the contract, so clarifying that prose does not invalidate history.
        if let Some(rest) = text.strip_prefix(HEAD) {
            let (_, body) = rest.split_once('\n')?;
            let (metadata, text) = body.split_once(WORDS)?;
            let written: Written = serde_json::from_str(metadata).ok()?;
            let envelope = Self { delivery: written.delivery, from: written.from, files: written.files, carried: None };
            return (!envelope.files.is_empty()).then_some((envelope, text));
        }
        let (_, body) = text.strip_prefix(HEAD_V2).or_else(|| text.strip_prefix(HEAD_V1))?.split_once('\n')?;
        let (metadata, text) = body.split_once(WORDS)?;
        let carried: AttachedMessage = serde_json::from_str(metadata).ok()?;
        (!carried.attachments.is_empty()).then(|| {
            let files = carried.attachments.iter().map(Told::of).collect();
            let envelope =
                Self { delivery: carried.delivery, from: carried.from, files, carried: Some(carried.attachments) };
            (envelope, text)
        })
    }

    /// The delivery with each file's receipt: the one an older envelope
    /// carried, or the one kept for its delivery. A file whose receipt is
    /// no longer kept is left out.
    pub fn attached(&self, receipts: &dyn Receipts) -> AttachedMessage {
        let attachments = match &self.carried {
            Some(carried) => carried.clone(),
            None => {
                let kept = receipts.of(&self.delivery);
                self.files.iter().filter_map(|told| kept.iter().find(|kept| kept.id == told.id).cloned()).collect()
            }
        };
        AttachedMessage { delivery: self.delivery.clone(), from: self.from.clone(), attachments }
    }

    /// The image receipts an older envelope carried. Newer ones go to a
    /// host as text alone, its pictures never in the host's record.
    pub fn carried_images(&self) -> impl Iterator<Item = &Attachment> {
        self.carried.iter().flatten().filter(|attachment| attachment.is_image())
    }

    /// Whether a picture a host was given from `path` is one of this
    /// envelope's files: the original at that very path, or, from a newer
    /// envelope, a file kept in the directory its id names (`<id>` or
    /// `<id>-2` and on).
    pub fn holds_picture(&self, path: &str) -> bool {
        if self.carried.is_some() {
            return self.carried_images().any(|attachment| attachment.path == path);
        }
        let Some(slot) =
            std::path::Path::new(path).parent().and_then(|slot| slot.file_name()).and_then(|slot| slot.to_str())
        else {
            return false;
        };
        self.files.iter().filter(|told| told.is_image()).any(|told| {
            let id = told.id.as_str();
            slot == id
                || slot.strip_prefix(id).and_then(|n| n.strip_prefix('-')).is_some_and(|n| n.parse::<u32>().is_ok())
        })
    }

    /// Whether this envelope tells of the file `id`.
    pub fn tells(&self, id: &NoticeId) -> bool {
        self.files.iter().any(|told| told.id == *id)
    }
}
