//! Host words are projected before splitting, without another conversation copy.

use fastctx_types::session::{Happened, SessionEvent, attachments, images};
use fastctx_types::{Address, ResidentId};

use crate::Node;

pub async fn codex(
    node: &std::sync::Arc<Node>,
    resident: &ResidentId,
    thread: &str,
    event: SessionEvent,
) -> std::io::Result<Vec<SessionEvent>> {
    if !matches!(&event.happened, Happened::Heard {message:Some(message),..} if message.detail["type"] == "userMessage"
        && message.content.iter().any(|block| matches!(block["type"].as_str(),Some("image"|"localImage"))))
    {
        return Ok(vec![event]);
    }
    let (about, _) = node.residents.about(resident).ok_or_else(|| std::io::Error::other("unknown picture resident"))?;
    let host = node.codex.of(node, &about).map_err(|error| std::io::Error::other(error.message))?;
    let mut events = host
        .locate_user_images(thread, &Address { machine: node.machine(), resident: resident.clone() }, event)
        .await
        .map_err(std::io::Error::other)?;
    crate::held_history::picture_fingerprints(node, &mut events)
        .await
        .map_err(|error| std::io::Error::other(error.message))?;
    Ok(events)
}

pub fn claude(node: &Node, resident: &ResidentId, event: &mut SessionEvent) {
    if let Happened::Heard { message: Some(message), .. } = &mut event.happened {
        images::claude(message, &Address { machine: node.machine(), resident: resident.clone() });
        let kept = images::kept(&message.content);
        if !kept.is_empty() {
            let represented: Vec<_> = message
                .content
                .iter()
                .filter(|block| block["type"] == "image")
                .map(|block| {
                    block["source"]["data"]
                        .as_str()
                        .and_then(|data| fastctx_hosts::image_digest::inline(data).ok())
                        .is_some_and(|(size, sha256)| {
                            kept.iter()
                                .any(|attachment| attachment.file.size == size && attachment.file.sha256 == sha256)
                        })
                })
                .collect();
            message.images.retain(|image| {
                let fastctx_types::attachments::AttachmentSource::HostImage {
                    host_image: fastctx_types::attachments::HostImage::Claude { image, .. },
                    ..
                } = &image.source
                else {
                    return true;
                };
                !represented[*image as usize]
            });
        }
    }
}

pub async fn live(node: &Node, resident: &ResidentId, mut event: SessionEvent) -> std::io::Result<()> {
    let claude_session = node.residents.about(resident).is_some_and(|(about, _)| about.host.agent.as_str() == "claude");
    if claude_session {
        claude(node, resident, &mut event);
        if let Happened::Heard { message: Some(message), .. } = &event.happened
            && !message.images.is_empty()
            && let Err(error) = crate::claude::wait_for_image(node, resident, &message.id).await
        {
            node.log.line(format_args!("Claude's user picture record for {}: {error}", message.id));
        }
    }
    crate::context::input::Projection::open(&node.world.conversation(resident))?.restore(&mut event)?;
    let restored = attachments::restore(event, &crate::attachments::delivered::of(node, resident));
    for event in restored {
        let Happened::Heard { text, message: Some(message), delivery, .. } = &event.happened else { continue };
        // The durable delivery already supplied the kept attachments. A
        // later host projection updates it only when it adds native pictures.
        let told = fastctx_hands::told::is_told(text) || fastctx_hosts::codex::sent_by_world(message);
        if (delivery.is_some() || told) && message.images.is_empty() {
            continue;
        }
        // The user's words the World handed in as the user's, which Claude
        // Code then keeps as the user's: the World's own record of them is
        // already shown (`claude::history::place` drops them the same way).
        if claude_session
            && message.images.is_empty()
            && attachments::Envelope::read(text).is_none()
            && node.claude.said_by_world(&node.world.conversation(resident), resident, text)
        {
            continue;
        }
        node.watchers.publish(resident, &event);
    }
    Ok(())
}

pub fn merge(recorded: &mut SessionEvent, hosted: &SessionEvent) {
    if let (Happened::Heard { message: Some(recorded), .. }, Happened::Heard { message: Some(hosted), .. }) =
        (&mut recorded.happened, &hosted.happened)
    {
        recorded.images = hosted.images.clone();
        recorded.content = hosted.content.clone();
        recorded.detail = hosted.detail.clone();
        recorded.presentation = hosted.presentation.clone();
        recorded.hook_fragments = hosted.hook_fragments.clone();
        recorded.client_id = hosted.client_id.clone();
    }
}
