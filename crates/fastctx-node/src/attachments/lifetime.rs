//! Originals keep their recorded paths until the last conversation using them is deleted.
//! Forks live on the original conversation's node, which answers for their references.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use fastctx_types::access::{Acting, HubAccount, HubId, Purpose};
use fastctx_types::attachments::Attachment;
use fastctx_types::ipc::Failure;
use fastctx_types::layout::Kind;
use fastctx_types::remote::{RemoteCall, RemoteReply};
use fastctx_types::{AccountName, Address, ResidentId};

use crate::Node;

pub(crate) const RETAINED: &str = "retained-attachments.json";

fn failed(error: impl std::fmt::Display) -> Failure {
    Failure::new(error.to_string())
}

/// Reads only durable conversation records; callers serialize this with inherited-reference writes.
fn readers(node: &Node, wanted: &[Attachment], excluding: Option<&ResidentId>) -> Result<Vec<bool>, Failure> {
    let mut used = vec![false; wanted.len()];
    for kept in node.world.kept().map_err(failed)? {
        let Kind::Home(id) = kept.kind else { continue };
        if excluding == Some(&id) || node.world.deleted().join(format!("{id}.json")).try_exists().map_err(failed)? {
            continue;
        }
        let Some(about) = super::references::optional::<fastctx_types::residents::About>(&kept.directory.about())?
        else {
            continue;
        };
        if about.id != id {
            return Err(failed("the conversation identity differs from its directory"));
        }
        let files = super::references::listed(node, &id)?;
        for (wanted, used) in wanted.iter().zip(&mut used) {
            *used |= files.iter().any(|file| same(file, wanted));
        }
    }
    Ok(used)
}

fn same(left: &Attachment, right: &Attachment) -> bool {
    left.id == right.id && left.file == right.file && left.conversation().resident == right.conversation().resident
}

pub(crate) fn referenced(node: &Node, wanted: &[Attachment]) -> Result<Vec<bool>, Failure> {
    let _transaction = crate::resident_storage::lock();
    readers(node, wanted, None)
}

/// A live inheriting conversation supplies the same account and login admission as the original did.
pub(crate) fn reader(node: &Node, wanted: &Attachment) -> Result<Option<ResidentId>, Failure> {
    let _transaction = crate::resident_storage::lock();
    for kept in node.world.kept().map_err(failed)? {
        let Kind::Home(id) = kept.kind else { continue };
        if node.world.deleted().join(format!("{id}.json")).try_exists().map_err(failed)? {
            continue;
        }
        if !kept.directory.about().try_exists().map_err(failed)? {
            continue;
        }
        if super::references::listed(node, &id)?.iter().any(|file| file == wanted) {
            return Ok(Some(id));
        }
    }
    Ok(None)
}

fn slots(place: &Path) -> Result<Vec<(PathBuf, Attachment)>, Failure> {
    let mut slots = Vec::new();
    let base = place.join("attachments");
    let entries = match std::fs::read_dir(base) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(slots),
        Err(error) => return Err(failed(error)),
    };
    for entry in entries {
        let entry = entry.map_err(failed)?;
        if !entry.file_type().map_err(failed)?.is_dir() {
            continue;
        }
        for name in ["receipt.json", "reference.json"] {
            if let Some(file) = super::references::optional(&entry.path().join(name))? {
                slots.push((entry.path(), file));
                break;
            }
        }
    }
    Ok(slots)
}

/// The authenticated account of a hub-directed guest-room deletion survives a restart.
pub(crate) fn remember(place: &Path, hub: &HubId, account: &AccountName) -> Result<(), Failure> {
    keep_marker(place, &Acting::Accounts { accounts: vec![HubAccount { hub: hub.clone(), account: account.clone() }] })
}

fn keep_marker(place: &Path, acting: &Acting) -> Result<(), Failure> {
    fastctx_platform::publish(
        &place.join(RETAINED),
        &serde_json::to_vec(acting).map_err(failed)?,
        fastctx_platform::Permissions::Private,
    )
    .map_err(failed)
}

/// Old original holders delete whole guest rooms. Only shared remote originals require this version check.
pub(crate) async fn ensure_holders(node: &Arc<Node>, id: &ResidentId) -> Result<(), Failure> {
    let Some((about, _)) = node.residents.about(id) else { return Ok(()) };
    let (files, used) = {
        let _transaction = crate::resident_storage::lock();
        let files = super::references::listed(node, id)?;
        let used = readers(node, &files, Some(id))?;
        (files, used)
    };
    let mut checked = Vec::new();
    for (file, used) in files.into_iter().zip(used) {
        if !used || node.is_me(&file.owner.machine) || checked.contains(&file.owner.machine) {
            continue;
        }
        let target = (&file.owner.machine).into();
        let answer = node.call(&about.acting, &target, Purpose::Look, RemoteCall::Facts).await.map_err(failed)??;
        if !matches!(answer, RemoteReply::Facts(facts) if facts.attachment_references) {
            return Err(failed(format!(
                "update {} before deleting a conversation whose forks use originals there",
                file.owner.machine
            )));
        }
        checked.push(file.owner.machine);
    }
    Ok(())
}

/// Query remote homes before taking any shared disk lock. An unanswered query never permits removal.
pub(crate) async fn remote_retained(node: &Arc<Node>, places: &[PathBuf]) -> Result<Vec<PathBuf>, Failure> {
    let mut retained = Vec::new();
    for place in places {
        let slots = slots(place)?;
        // A room named by a tag of its home learns it from what it holds.
        let owner = match place.file_name().and_then(|name| name.to_str()).and_then(Kind::of) {
            Some(Kind::Guest(owner)) => owner,
            Some(Kind::Visiting { .. }) => match slots.first() {
                Some((_, file)) => file.conversation().clone(),
                None => continue,
            },
            _ => continue,
        };
        if slots.is_empty() {
            continue;
        }
        // A machine no hub knows any more keeps no conversation that could
        // still read them: they go with the room.
        if !node.links().iter().any(|link| link.view().member(&owner.machine).is_some()) {
            continue;
        }
        let acting = super::references::optional::<Acting>(&place.join(RETAINED))?.unwrap_or(Acting::Here);
        let target = (&owner.machine).into();
        let facts = node.call(&acting, &target, Purpose::Look, RemoteCall::Facts).await.map_err(failed)??;
        if !matches!(facts, RemoteReply::Facts(facts) if facts.attachment_references) {
            return Err(failed("update the conversation's machine before collecting its shared attachments"));
        }
        let files = slots.iter().map(|(_, file)| file.clone()).collect();
        let answer = node
            .call(
                &acting,
                &target,
                Purpose::Look,
                RemoteCall::AttachmentReferences { conversation: owner, attachments: files },
            )
            .await
            .map_err(failed)??;
        let RemoteReply::AttachmentReferences { used } = answer else {
            return Err(failed("expected attachment references"));
        };
        if used.len() != slots.len() {
            return Err(failed("the attachment reference count differs from the request"));
        }
        retained.extend(slots.into_iter().zip(used).filter_map(|((path, _), used)| used.then_some(path)));
    }
    Ok(retained)
}

/// Under the resident-storage transaction, remove an attachment directory except its live originals.
pub(crate) fn remove(node: &Node, place: &Path, remote: &[PathBuf]) -> Result<bool, Failure> {
    let slots = slots(place)?;
    let kind = place.file_name().and_then(|name| name.to_str()).and_then(Kind::of);
    let kept = match kind {
        Some(Kind::Guest(_) | Kind::Visiting { .. }) => slots.iter().map(|(path, _)| remote.contains(path)).collect(),
        Some(Kind::Home(ref id)) => {
            readers(node, &slots.iter().map(|(_, file)| file.clone()).collect::<Vec<_>>(), Some(id))?
        }
        _ => vec![false; slots.len()],
    };
    let kept: Vec<PathBuf> = slots.into_iter().zip(kept).filter_map(|((path, _), used)| used.then_some(path)).collect();
    let base = place.join("attachments");
    let entries = match std::fs::read_dir(&base) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(failed(error)),
    };
    for entry in entries {
        let path = entry.map_err(failed)?.path();
        if !kept.contains(&path) {
            fastctx_platform::owned_directory::delete(&path).map_err(failed)?;
        }
    }
    if kept.is_empty() {
        std::fs::remove_dir(base).map_err(failed)?;
        Ok(false)
    } else {
        if !place.join(RETAINED).try_exists().map_err(failed)? {
            keep_marker(place, &Acting::Here)?;
        }
        Ok(true)
    }
}

/// Deleted original conversations whose attachments this conversation still references.
pub(crate) fn releasing(node: &Node, id: &ResidentId) -> Result<Vec<Address>, Failure> {
    let mut owners = Vec::new();
    for file in super::references::listed(node, id)? {
        let owner = file.conversation();
        if owner.resident != *id
            && node.is_me(&owner.machine)
            && node.world.conversation(&owner.resident).path().join(RETAINED).try_exists().map_err(failed)?
            && !owners.contains(owner)
        {
            owners.push(owner.clone());
        }
    }
    Ok(owners)
}

/// Retry preserved guest rooms when their authoritative conversation node is reachable again.
pub(crate) async fn collect(node: &Arc<Node>) {
    let kept = match node.world.kept() {
        Ok(kept) => kept,
        Err(error) => {
            node.log.line(format_args!("finding shared attachment originals: {error}"));
            return;
        }
    };
    for kept in kept {
        match kept.directory.path().join(RETAINED).try_exists() {
            Ok(true) => {
                if let Err(error) = crate::data::delete::delete_one(node, &kept.name).await {
                    node.log.line(format_args!("collecting shared attachments of {}: {}", kept.name, error.why()));
                }
            }
            Ok(false) => {}
            Err(error) => node.log.line(format_args!("reading attachment retention for {}: {error}", kept.name)),
        }
    }
}
