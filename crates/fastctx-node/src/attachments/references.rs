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

/// The small picture kept at home of an attachment whose original is kept
/// elsewhere. Its conversation deleted, a conversation forked from it keeps
/// the picture along with the attachment (`keep_inherited`, `hand_previews`),
/// and is asked: only the forks of that conversation, not every one here.
pub(crate) fn preview(node: &Node, wanted: &Attachment) -> Result<fastctx_types::thumbnail::Thumbnail, Failure> {
    let resident = &wanted.conversation().resident;
    let own = reference(node, resident, wanted).and_then(|kept| {
        kept.ok_or_else(|| failure("this attachment has no registered original"))?;
        read_json(&preview_in(node, resident, wanted))
    });
    let Err(missing) = own else { return own };
    for heir in forks_of(node, wanted.conversation()) {
        let path = preview_in(node, &heir, wanted);
        if fastctx_platform::disk::is_there(&path)
            && matches!(super::index::reference(&node.world.conversation(&heir), wanted), Ok(Some(_)))
            && let Ok(preview) = read_json(&path)
        {
            return Ok(preview);
        }
    }
    Err(missing)
}

/// The conversations here forked from `conversation`, archived ones too.
fn forks_of(node: &Node, conversation: &fastctx_types::Address) -> Vec<ResidentId> {
    if !node.is_me(&conversation.machine) {
        return Vec::new();
    }
    node.residents.everyone().filter(|one| one.about.forked_from.as_ref() == Some(conversation)).map(|one| one.id).collect()
}

/// Gives the forks of `parent` the small pictures of its attachments kept
/// elsewhere that they hold too and have no picture of yet, as `parent` is
/// deleted: forks made before they kept their own (`keep_inherited`) would
/// lose them with it.
pub(crate) fn hand_previews(node: &Node, parent: &ResidentId) {
    let conversation = fastctx_types::Address { machine: node.machine(), resident: parent.clone() };
    let forks = forks_of(node, &conversation);
    if forks.is_empty() {
        return;
    }
    let Ok(listed) = listed(node, parent) else { return };
    for file in listed.iter().filter(|file| !node.is_me(&file.owner.machine)) {
        for fork in &forks {
            if matches!(super::index::reference(&node.world.conversation(fork), file), Ok(Some(_))) {
                copy_preview(node, parent, fork, file);
            }
        }
    }
}

/// Copies the small picture of `file` from `from`'s directory to `to`'s,
/// whole or not at all, where `to` has none yet.
fn copy_preview(node: &Node, from: &ResidentId, to: &ResidentId, file: &Attachment) {
    let (source, target) = (preview_in(node, from, file), preview_in(node, to, file));
    if !fastctx_platform::disk::is_there(&source) || fastctx_platform::disk::is_there(&target) {
        return;
    }
    let copied = target.parent().map(fastctx_platform::ensure_private_dir).transpose().and_then(|_| {
        let bytes = fastctx_platform::disk::read(&source)?;
        fastctx_platform::publish(&target, &bytes, fastctx_platform::Permissions::Private)
    });
    if let Err(error) = copied {
        node.log.line(format_args!("keeping a forked attachment's small picture: {error}"));
    }
}

fn preview_in(node: &Node, resident: &ResidentId, wanted: &Attachment) -> PathBuf {
    node.world.conversation(resident).attachments().join(wanted.id.as_str()).join("preview.json")
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
    super::index::inherit(&node.world.conversation(parent), &node.world.conversation(child), retained).map_err(failure)?;
    // The small picture of one kept elsewhere goes with it, so the fork shows
    // it after the conversation it came from is deleted.
    for file in retained.iter().filter(|file| !node.is_me(&file.owner.machine)) {
        copy_preview(node, parent, child, file);
    }
    Ok(())
}

pub(super) fn optional<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Option<T>, Failure> {
    match fastctx_platform::disk::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map(Some).map_err(failure),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(failure(error)),
    }
}
