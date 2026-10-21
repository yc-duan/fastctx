//! Verified references and the single small history image retained at home.

use super::*;

pub(crate) fn reference(
    node: &Node,
    resident: &ResidentId,
    wanted: &Attachment,
) -> Result<Option<Attachment>, Failure> {
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
    let retained: Vec<_> = events
        .iter()
        .cloned()
        .flat_map(restore)
        .flat_map(|event| match event.happened {
            Happened::Heard { message: Some(message), .. } => message.attachments,
            _ => Vec::new(),
        })
        .collect();
    keep_inherited(node, parent, child, &retained)
}

pub(crate) fn inherit_texts(
    node: &Node,
    parent: &ResidentId,
    child: &ResidentId,
    messages: &[serde_json::Value],
) -> Result<(), Failure> {
    let retained: Vec<_> = messages
        .iter()
        .filter(|message| message["role"] == "user")
        .filter_map(|message| fastctx_types::session::attachments::AttachedMessage::read(message["text"].as_str()?))
        .flat_map(|(message, _)| message.attachments)
        .collect();
    keep_inherited(node, parent, child, &retained)
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
