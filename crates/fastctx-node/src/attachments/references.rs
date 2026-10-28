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
    let room = node.world.conversation(resident);
    let path = room.attachments().join(wanted.id.as_str()).join("reference.json");
    if let Some(kept) = optional::<Attachment>(&path)? {
        if kept != *wanted {
            return Err(failure("the attachment differs from its registered original"));
        }
        return Ok(Some(kept));
    }
    let inherited = optional::<Vec<Attachment>>(&room.attachments().join("inherited.json"))?.unwrap_or_default();
    Ok(inherited.into_iter().find(|kept| kept == wanted))
}

pub(crate) fn preview(node: &Node, wanted: &Attachment) -> Result<fastctx_types::thumbnail::Thumbnail, Failure> {
    let resident = &wanted.conversation().resident;
    reference(node, resident, wanted)?.ok_or_else(|| failure("this attachment has no registered original"))?;
    read_json(&node.world.conversation(resident).attachments().join(wanted.id.as_str()).join("preview.json"))
}

pub(crate) fn listed(node: &Node, resident: &ResidentId) -> Result<Vec<Attachment>, Failure> {
    let base = node.world.conversation(resident).attachments();
    let mut files = optional::<Vec<Attachment>>(&base.join("inherited.json"))?.unwrap_or_default();
    let entries = match std::fs::read_dir(&base) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(files),
        Err(error) => return Err(failure(error)),
    };
    for entry in entries {
        let entry = entry.map_err(failure)?;
        if !entry.file_type().map_err(failure)?.is_dir() {
            continue;
        }
        for name in ["receipt.json", "reference.json"] {
            if let Some(file) = optional::<Attachment>(&entry.path().join(name))?
                && !files.contains(&file)
            {
                files.push(file);
            }
        }
    }
    Ok(files)
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
    let _transaction = crate::resident_storage::lock();
    if node.world.deleted().join(format!("{parent}.json")).try_exists().map_err(failure)?
        || node.world.deleted().join(format!("{child}.json")).try_exists().map_err(failure)?
    {
        return Err(failure("the source conversation was deleted before its attachments were inherited"));
    }
    let files: Vec<_> = listed(node, parent)?.into_iter().filter(|file| retained.contains(file)).collect();
    if files.is_empty() {
        return Ok(());
    }
    let base = node.world.conversation(child).attachments();
    fastctx_platform::ensure_private_dir(&base).map_err(failure)?;
    fastctx_platform::publish(
        &base.join("inherited.json"),
        &serde_json::to_vec(&files).map_err(failure)?,
        fastctx_platform::Permissions::Private,
    )
    .map_err(failure)
}

pub(super) fn optional<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Option<T>, Failure> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map(Some).map_err(failure),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(failure(error)),
    }
}
