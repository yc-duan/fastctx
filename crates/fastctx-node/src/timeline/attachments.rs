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
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};

/// How many times an attachment was placed in each conversation since this
/// node began.
static PLACED: LazyLock<Mutex<HashMap<ResidentId, u64>>> = LazyLock::new(Mutex::default);

/// The conversations whose attachments were looked at whole since this node
/// began: as many placed as then, in which holding period. One placed
/// meanwhile, or a period held anew, is looked at again (MM16, K15).
static LOOKED: LazyLock<Mutex<HashMap<ResidentId, (u64, i64)>>> = LazyLock::new(Mutex::default);

/// The most attachments one commit writes.
const AT_ONCE: usize = 64;

fn failure(error: impl std::fmt::Display) -> Failure {
    Failure::new(error.to_string())
}

/// An attachment was placed in `resident`'s directory: looked at again.
pub(crate) fn placed_here(node: &Node, resident: &ResidentId) {
    *PLACED.lock().expect("placed attachments").entry(resident.clone()).or_default() += 1;
    super::tail::wanted(node, resident);
}

/// Each attachment folder of `directory` with its receipt — or the
/// reference of one kept elsewhere (MM17) — and its original when kept
/// here: the node keeps it as `content.<ext>`, or where its receipt says,
/// never whatever else lies beside it (MM18).
/// The attachments kept of `room`, and whether any could not be read now:
/// then they are looked at again (L07). Those its index holds — referred
/// to, or inherited from the conversation it was forked from — come in
/// too, described, with their original where it lies here (NN10, L06).
/// An attachment kept: its slot, its receipt and its original when here.
type Found = (String, Attachment, Option<PathBuf>);

/// Whether an original is at `path`: not there is no; one that cannot be
/// looked at is an error, tried again later, never taken as none (N11).
fn present(path: &std::path::Path) -> std::io::Result<bool> {
    match fastctx_platform::disk::metadata(path) {
        Ok(metadata) => Ok(metadata.is_file()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn kept(room: &fastctx_types::layout::ConversationDir) -> std::io::Result<(Vec<Found>, bool)> {
    let directory = room.attachments();
    let mut found = Vec::new();
    let mut unread = false;
    let entries = match fastctx_platform::disk::read_dir(&directory) {
        Ok(entries) => Some(entries),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    for entry in entries.into_iter().flatten() {
        let entry = entry?;
        let folder = entry.path();
        let Some(slot) = folder.file_name().and_then(|name| name.to_str()).map(str::to_string) else { continue };
        let (bytes, referenced) = match fastctx_platform::disk::read(folder.join("receipt.json")) {
            Ok(bytes) => (bytes, false),
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                unread = true;
                continue;
            }
            Err(_) => match fastctx_platform::disk::read(folder.join("reference.json")) {
                Ok(bytes) => (bytes, true),
                Err(error) => {
                    unread |= error.kind() != std::io::ErrorKind::NotFound;
                    continue;
                }
            },
        };
        let Ok(attachment) = serde_json::from_slice::<Attachment>(&bytes) else {
            unread = true;
            continue;
        };
        let named = Path::new(&attachment.path).file_name().map(|name| folder.join(name));
        let original = if referenced {
            None
        } else {
            fastctx_platform::disk::read_dir(&folder)?
                .filter_map(Result::ok)
                .map(|file| file.path())
                .find(|path| {
                    path.file_name().and_then(|name| name.to_str()).is_some_and(|name| name.starts_with("content."))
                })
                .or(named)
                .map(|path| present(&path).map(|here| here.then_some(path)))
                .transpose()?
                .flatten()
        };
        found.push((slot, attachment, original));
    }
    // What the index holds that no folder here keeps: by its id, the
    // original where its receipt says it lies, if that is here.
    let known: std::collections::HashSet<String> = found.iter().map(|(slot, _, _)| slot.clone()).collect();
    for attachment in crate::attachments::index::listed(room)? {
        let slot = attachment.id.to_string();
        if known.contains(&slot) || found.iter().any(|(id, _, _)| *id == slot) {
            continue;
        }
        let path = PathBuf::from(&attachment.path);
        let original = present(&path)?.then_some(path);
        found.push((slot, attachment, original));
    }
    found.sort_by(|a, b| a.0.cmp(&b.0));
    Ok((found, unread))
}

/// Brings `resident`'s attachments into the asset library: those it lacks,
/// each with its original when it is kept here; how many were written.
pub(super) async fn produce(
    node: &Arc<Node>,
    resident: &ResidentId,
    producing: &super::serial::Serial,
) -> Result<usize, Failure> {
    let placed_count = PLACED.lock().expect("placed attachments").get(resident).copied().unwrap_or(0);
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
            asking.is_live(&looked)? && asking.is_pulled(&looked)?,
            asking.orderer(&looked, ConversationAttachment::STREAM)?,
        ))
    })
    .await
    .map_err(failure)?;
    // Not before the scope is held here; for one from elsewhere, not before
    // the hub described it (AG10); ordered elsewhere, taken over first.
    let Some(holding) = holding else { return Ok(0) };
    if LOOKED.lock().expect("looked attachments").get(resident) == Some(&(placed_count, holding)) {
        return Ok(0);
    }
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
    let signer = assets.signer();
    let written = fastctx_platform::task::blocking(move || {
        let _held = held;
        let writer = Writer::<ConversationAttachment>::new(&library, &scope, ConversationAttachment::STREAM)
            .map_err(failure)?
            .in_holding(holding);
        let (mut writes, mut leases, mut written) = (Vec::new(), Vec::new(), 0);
        let (found, unread) = kept(&room).map_err(failure)?;
        for (slot, attachment, original) in found {
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
        Ok::<_, Failure>((written, unread))
    })
    .await?;
    let (written, unread) = written;
    // Looked at whole only when everything could be read (L07); what could
    // not be is looked at again soon, the job asked again (M08).
    if unread {
        return Err(failure(format!("{resident}'s attachments: some could not be read now; {written} written")));
    }
    LOOKED.lock().expect("looked attachments").insert(resident.clone(), (placed_count, holding));
    Ok(written)
}
