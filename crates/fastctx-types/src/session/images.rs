//! Native user pictures keep their host coordinates across World envelopes.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::attachments::{Attachment, AttachmentSource, HostImage};
use crate::{Address, NoticeId};

use super::UserMessage;
use super::attachments::Envelope;

/// A user picture not already represented by a kept attachment. Unknown
/// display metadata stays null; the source is passed unchanged to image reads.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserImage {
    pub source: AttachmentSource,
    pub name: Option<String>,
    pub media_type: Option<String>,
    /// The picture's bytes, where its machine read them out of the host's
    /// record: a viewer keeps it by this and fetches it once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<super::Fingerprint>,
}

struct Block<'a> {
    value: &'a Value,
    delivery: Option<NoticeId>,
    kept: bool,
}

fn blocks<'a>(content: &'a [Value], attachments: &[Attachment], codex: bool) -> Vec<Block<'a>> {
    let envelopes: Vec<_> = content
        .iter()
        .enumerate()
        .filter_map(|(index, block)| Some((index, Envelope::read(block["text"].as_str()?)?.0)))
        .collect();
    let kept = |path: &str| {
        attachments.iter().any(|attachment| attachment.is_image() && attachment.path == path)
            || envelopes.iter().any(|(_, envelope)| envelope.holds_picture(path))
    };
    content
        .iter()
        .enumerate()
        .filter(|(_, block)| block["type"] == "image" || codex && block["type"] == "localImage")
        .map(|(index, value)| {
            let envelope = envelopes.iter().rev().find(|(start, _)| *start <= index).or_else(|| envelopes.first());
            let delivery = envelope.map(|(_, envelope)| envelope.delivery.clone());
            let represented = codex && value["type"] == "localImage" && value["path"].as_str().is_some_and(kept);
            Block { value, delivery, kept: represented }
        })
        .collect()
}

pub fn codex(message: &mut UserMessage, owner: &Address, turn: &str, submission: u32, legacy: bool) {
    let pictures = blocks(&message.content, &message.attachments, true);
    let inline = pictures.iter().filter(|block| block.value["type"] == "image").count();
    let mut inline_index = 0usize;
    let mut local_index = 0usize;
    message.images = pictures
        .into_iter()
        .enumerate()
        .filter_map(|(original_index, block)| {
            let local = block.value["type"] == "localImage";
            let legacy_index = if local {
                let index = inline + local_index;
                local_index += 1;
                index
            } else {
                let index = inline_index;
                inline_index += 1;
                index
            };
            if block.kept {
                return None;
            }
            let image = u32::try_from(if legacy { legacy_index } else { original_index })
                .expect("a host image ordinal fits u32");
            let name = if local {
                block.value["path"]
                    .as_str()
                    .and_then(|path| std::path::Path::new(path).file_name())
                    .and_then(|name| name.to_str())
                    .map(str::to_owned)
            } else {
                block.value["name"].as_str().map(str::to_owned)
            };
            let media_type = block.value["url"]
                .as_str()
                .and_then(|url| url.strip_prefix("data:"))
                .and_then(|data| data.split_once(';'))
                .map(|(mime, _)| mime.to_owned());
            Some(UserImage {
                source: AttachmentSource::HostImage {
                    owner: owner.clone(),
                    host_image: HostImage::Codex { turn: turn.into(), message: submission, image },
                },
                name,
                media_type,
                content: None,
            })
        })
        .collect();
}

pub fn claude(message: &mut UserMessage, owner: &Address) {
    message.images = blocks(&message.content, &message.attachments, false)
        .into_iter()
        .enumerate()
        .filter(|(_, block)| !block.kept)
        .map(|(image, block)| UserImage {
            source: AttachmentSource::HostImage {
                owner: owner.clone(),
                host_image: HostImage::Claude {
                    message: message.id.clone(),
                    image: u32::try_from(image).expect("a host image ordinal fits u32"),
                },
            },
            name: block.value["name"].as_str().map(str::to_owned),
            media_type: block.value["source"]["media_type"].as_str().map(str::to_owned),
            content: None,
        })
        .collect();
}

/// Each native picture belongs to one restored delivery; its host ordinal
/// was assigned before any split or removal of kept attachment pictures.
pub(super) fn delivered(message: &UserMessage) -> Vec<(Option<NoticeId>, UserImage)> {
    let codex = message.images.first().is_some_and(|image| {
        matches!(image.source, AttachmentSource::HostImage { host_image: HostImage::Codex { .. }, .. })
    });
    let blocks = blocks(&message.content, &message.attachments, codex);
    if codex {
        blocks
            .into_iter()
            .filter(|block| !block.kept)
            .zip(message.images.iter().cloned())
            .map(|(block, image)| (block.delivery, image))
            .collect()
    } else {
        message
            .images
            .iter()
            .filter_map(|image| {
                let AttachmentSource::HostImage { host_image: HostImage::Claude { image: ordinal, .. }, .. } =
                    &image.source
                else {
                    return None;
                };
                Some((blocks.get(*ordinal as usize)?.delivery.clone(), image.clone()))
            })
            .collect()
    }
}

pub fn kept(content: &[Value]) -> Vec<Attachment> {
    content
        .iter()
        .filter_map(|block| Envelope::read(block["text"].as_str()?))
        .flat_map(|(envelope, _)| envelope.carried_images().cloned().collect::<Vec<_>>())
        .collect()
}
