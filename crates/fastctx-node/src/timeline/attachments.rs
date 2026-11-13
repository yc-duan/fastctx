//! The files the user attached to a conversation in the asset library
//! (design §28.16, plan 3-4 group 2): each attachment kept in the
//! conversation's directory, described, with its original as an object when
//! the original is kept here — then on every device that holds the
//! conversation, and on the hub for good. Looked at by the timeline's job
//! once after the node starts and again whenever an attachment is placed;
//! only what the library lacks is written. Made by the machine the
//! conversation lives on, as its metadata is.

use super::assets::placed;
use crate::Node;
use fastctx_content::v2::{ConversationAttachment, Kind as _, LEASE, Write, Writer};
use fastctx_types::ResidentId;
use fastctx_types::attachments::Attachment;
use fastctx_types::ipc::Failure;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};

/// The conversations whose attachments were looked at since this node
/// began, with nothing placed since.
static LOOKED: LazyLock<Mutex<HashSet<ResidentId>>> = LazyLock::new(Mutex::default);

/// The most attachments one commit writes.
const AT_ONCE: usize = 64;

/// The files of an attachment's folder that are not its original.
const BESIDE: [&str; 3] = ["receipt.json", "reference.json", "upload.part"];

fn failure(error: impl std::fmt::Display) -> Failure {
    Failure::new(error.to_string())
}

/// An attachment was placed in `resident`'s directory: looked at again.
pub(crate) fn placed_here(node: &Node, resident: &ResidentId) {
    LOOKED.lock().expect("looked attachments").remove(resident);
    super::tail::wanted(node, resident);
}

/// Each attachment folder of `directory` with its receipt.
fn kept(directory: &Path) -> std::io::Result<Vec<(String, Attachment, Option<PathBuf>)>> {
    let mut found = Vec::new();
    let entries = match fastctx_platform::disk::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(found),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = entry?;
        let folder = entry.path();
        let Some(slot) = folder.file_name().and_then(|name| name.to_str()).map(str::to_string) else { continue };
        let Ok(bytes) = fastctx_platform::disk::read(folder.join("receipt.json")) else { continue };
        let Ok(attachment) = serde_json::from_slice::<Attachment>(&bytes) else { continue };
        let original =
            fastctx_platform::disk::read_dir(&folder)?.filter_map(Result::ok).map(|file| file.path()).find(|path| {
                path.file_name().and_then(|name| name.to_str()).is_some_and(|name| !BESIDE.contains(&name))
                    && fastctx_platform::disk::is_file(path)
            });
        found.push((slot, attachment, original));
    }
    found.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(found)
}

/// Brings `resident`'s attachments into the asset library: those it lacks,
/// each with its original when it is kept here; how many were written.
pub(super) async fn produce(
    node: &Arc<Node>,
    resident: &ResidentId,
    producing: &super::serial::Serial,
) -> Result<usize, Failure> {
    if LOOKED.lock().expect("looked attachments").contains(resident) {
        return Ok(0);
    }
    // Held by each piece of blocking work too (LL16).
    let held = producing.hold(resident).await;
    let Some(assets) = node.assets.as_ref() else { return Ok(0) };
    let Some((about, _)) = node.residents.about(resident) else { return Ok(0) };
    let Some(placed) = placed(node, &about)? else { return Ok(0) };
    let room = node.world.conversation(resident);
    let (library, scope) = (assets.library.clone(), placed.scope.clone());
    if crate::assets::release::released_away(room.path(), &scope, &assets.device()) {
        return Ok(0);
    }
    let (asking, looked, holds) = (library.clone(), scope.clone(), held.clone());
    let (holding, live, orderer) = fastctx_platform::task::blocking(move || {
        let _held = holds;
        Ok::<_, fastctx_content::Error>((
            asking.holding(&looked)?,
            asking.is_live(&looked)?,
            asking.orderer(&looked, ConversationAttachment::STREAM)?,
        ))
    })
    .await
    .map_err(failure)?;
    // Not before the scope is held here; for one from elsewhere, not before
    // the hub described it (AG10); ordered elsewhere, taken over first.
    let Some(holding) = holding else { return Ok(0) };
    let born_elsewhere = node.residents.birth_machine(resident).is_some_and(|born| !node.is_me(&born));
    if born_elsewhere && !live {
        return Ok(0);
    }
    if orderer.is_some_and(|orderer| orderer != assets.device()) {
        crate::assets::takeover::want(node, &placed.hub, &scope, ConversationAttachment::STREAM);
        return Ok(0);
    }
    // A machine assisting another account keeps no original but pictures:
    // the others are described alone here (`world-network.md`).
    let pictures_only = crate::assists::assisting(node, &about.acting);
    let (signer, directory) = (assets.signer(), room.attachments());
    let written = fastctx_platform::task::blocking(move || {
        let _held = held;
        let writer = Writer::<ConversationAttachment>::new(&library, &scope, ConversationAttachment::STREAM)
            .map_err(failure)?
            .in_holding(holding);
        let (mut writes, mut leases, mut written) = (Vec::new(), Vec::new(), 0);
        for (slot, attachment, original) in kept(&directory).map_err(failure)? {
            let item = slot.into_bytes();
            if library.item(&scope, ConversationAttachment::STREAM, &item).map_err(failure)?.is_some() {
                continue;
            }
            let mut objects = Vec::new();
            let image = attachment.file.media_type.starts_with("image/");
            if let Some(path) = original.filter(|_| image || !pictures_only) {
                let mut file = fastctx_platform::disk::open(&path).map_err(failure)?;
                let lease = library.put(&mut file, "attachments", LEASE).map_err(failure)?;
                let hash: String = lease.object.hash.iter().map(|byte| format!("{byte:02x}")).collect();
                // Not the bytes its receipt describes: described alone.
                if lease.object.size == attachment.file.size && hash.eq_ignore_ascii_case(&attachment.file.sha256) {
                    objects.push(lease.object);
                    leases.push(lease);
                }
            }
            let body = serde_json::to_vec(&serde_json::json!({
                "id": attachment.id,
                "name": attachment.file.name,
                "media_type": attachment.file.media_type,
                "size": attachment.file.size,
                "sha256": attachment.file.sha256,
                "image": image,
                "owner": attachment.owner,
            }))
            .map_err(failure)?;
            writes.push(Write::<ConversationAttachment> { order: item.clone(), item, value: Some(body), objects });
            if writes.len() >= AT_ONCE {
                written += writes.len();
                writer
                    .write_all(std::mem::take(&mut writes), std::mem::take(&mut leases), false, signer.clone())
                    .map_err(failure)?;
            }
        }
        if !writes.is_empty() {
            written += writes.len();
            writer.write_all(writes, leases, false, signer).map_err(failure)?;
        }
        Ok::<_, Failure>(written)
    })
    .await?;
    LOOKED.lock().expect("looked attachments").insert(resident.clone());
    Ok(written)
}
