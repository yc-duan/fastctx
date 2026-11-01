//! Verified references and the single small history image retained at home.

use super::*;

pub(crate) fn reference(
    node: &Node,
    resident: &ResidentId,
    wanted: &Attachment,
) -> Result<Option<Attachment>, Failure> {
    if let Some((original, _)) = super::forwarded::original(node, wanted)? {
        if original.conversation().resident != *resident {
            return Err(failure("the forwarded original belongs to another conversation"));
        }
        return Ok(Some(original));
    }
    super::index::reference(&node.world.conversation(resident), wanted).map_err(failure)
}

pub(crate) fn preview(node: &Node, wanted: &Attachment) -> Result<fastctx_types::thumbnail::Thumbnail, Failure> {
    let resident = &wanted.conversation().resident;
    reference(node, resident, wanted)?.ok_or_else(|| failure("this attachment has no registered original"))?;
    read_json(&node.world.conversation(resident).attachments().join(wanted.id.as_str()).join("preview.json"))
}

pub(crate) fn listed(node: &Node, resident: &ResidentId) -> Result<Vec<Attachment>, Failure> {
    super::index::listed(&node.world.conversation(resident)).map_err(failure)
}

pub(crate) fn inherit(
    node: &Node,
    parent: &ResidentId,
    child: &ResidentId,
    events: &[fastctx_types::session::SessionEvent],
) -> Result<(), Failure> {
    use fastctx_types::session::{Happened, attachments::restore};
    let receipts = super::delivered::of(node, parent);
    let deliveries: Vec<_> = events
        .iter()
        .cloned()
        .flat_map(|event| restore(event, &receipts))
        .filter_map(|event| match event.happened {
            Happened::Heard { delivery: Some(delivery), message: Some(message), .. } => {
                Some((delivery, message.attachments))
            }
            _ => None,
        })
        .collect();
    keep_deliveries(node, parent, child, deliveries)
}

pub(crate) fn inherit_texts(
    node: &Node,
    parent: &ResidentId,
    child: &ResidentId,
    messages: &[serde_json::Value],
) -> Result<(), Failure> {
    let receipts = super::delivered::of(node, parent);
    let deliveries: Vec<_> = messages
        .iter()
        .filter(|message| message["role"] == "user")
        .filter_map(|message| fastctx_types::session::attachments::Envelope::read(message["text"].as_str()?))
        .map(|(envelope, _)| {
            let attached = envelope.attached(&receipts);
            (attached.delivery, attached.attachments)
        })
        .collect();
    keep_deliveries(node, parent, child, deliveries)
}

/// Keeps for `child` the files of the deliveries its history brings from
/// `parent`, and the receipts its host's record will name them by.
fn keep_deliveries(
    node: &Node,
    parent: &ResidentId,
    child: &ResidentId,
    deliveries: Vec<(fastctx_types::NoticeId, Vec<Attachment>)>,
) -> Result<(), Failure> {
    let retained: Vec<_> = deliveries.iter().flat_map(|(_, attachments)| attachments.iter().cloned()).collect();
    keep_inherited(node, parent, child, &retained)?;
    super::delivered::inherit(&node.world.conversation(child), deliveries).map_err(failure)
}

fn keep_inherited(
    node: &Node,
    parent: &ResidentId,
    child: &ResidentId,
    retained: &[Attachment],
) -> Result<(), Failure> {
    node.residents.active(parent)?;
    if node.residents.deleted(child).is_some() || node.residents.is_deleting(child) {
        return Err(failure("the destination conversation was deleted before its attachments were inherited"));
    }
    super::index::inherit(&node.world.conversation(parent), &node.world.conversation(child), retained).map_err(failure)
}

pub(super) fn optional<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Option<T>, Failure> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map(Some).map_err(failure),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(failure(error)),
    }
}
