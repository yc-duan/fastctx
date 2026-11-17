//! A conversation's input-box draft in the asset library (design §28.24,
//! plan 3-4 group 4): the composer here merges every device's edits, so what
//! goes in is its view as it stands — the text, the attachments and uploads
//! with it, and the epoch a send moves on — one item, written where it
//! changed. Looked at by the timeline's job, which each committed change of
//! the composer asks for, and once for each conversation after the node
//! starts.

use super::beside::{Beside, writable};
use crate::Node;
use fastctx_content::v2::{ConversationDraft, Kind as _, Write, Writer};
use fastctx_types::ResidentId;
use fastctx_types::ipc::Failure;
use std::collections::HashSet;
use std::sync::{Arc, LazyLock, Mutex};

/// The conversations whose composer changed since their draft was written,
/// and those whose draft was looked at since the node began.
static CHANGED: LazyLock<Mutex<HashSet<ResidentId>>> = LazyLock::new(Mutex::default);
static LOOKED: LazyLock<Mutex<std::collections::HashMap<ResidentId, i64>>> = LazyLock::new(Mutex::default);

/// The one item a draft is.
const ITEM: &[u8] = b"draft";

/// The most a body holds as it is.
const BODY_MOST: usize = fastctx_content::RECORD_BYTES - 4096;

fn failure(error: impl std::fmt::Display) -> Failure {
    Failure::new(error.to_string())
}

/// `resident`'s composer changed: its draft is written again.
pub(crate) fn changed(node: &Node, resident: &ResidentId) {
    CHANGED.lock().expect("changed drafts").insert(resident.clone());
    super::tail::wanted(node, resident);
}

/// Brings `resident`'s draft in the asset library to its composer here;
/// how many items were written.
pub(super) async fn produce(
    node: &Arc<Node>,
    resident: &ResidentId,
    producing: &super::serial::Serial,
) -> Result<usize, Failure> {
    // A resident removed from the World keeps no draft to bring: its
    // composer refuses it, and asking again every few seconds would refuse
    // it for good.
    if node.residents.removed(resident).is_some() {
        CHANGED.lock().expect("changed drafts").remove(resident);
        return Ok(0);
    }
    let changed = CHANGED.lock().expect("changed drafts").remove(resident);
    let again = || {
        if changed {
            CHANGED.lock().expect("changed drafts").insert(resident.clone());
        }
    };
    // Where it would go, and whether it is looked at at all: the
    // conversation's producers held only for this, never while the composer
    // is read, which waits on the conversation's gate the words take with
    // these held (OO03).
    let holding = {
        let held = producing.hold(resident).await;
        match writable(node, resident, ConversationDraft::STREAM, &held).await {
            Ok(Some(Beside { holding, .. })) => holding,
            // Looked at again once it may be written (OO21).
            Ok(None) => {
                again();
                return Ok(0);
            }
            Err(error) => {
                again();
                return Err(error);
            }
        }
    };
    // Looked at in this holding period and not changed since: nothing; held
    // anew, it is written again (M22).
    if !changed && LOOKED.lock().expect("looked drafts").get(resident) == Some(&holding) {
        return Ok(0);
    }
    let read = crate::composer::here(node, &node.machine(), resident, fastctx_types::composer::Request::Read).await;
    let composer = match read {
        Ok(updated) => updated.composer,
        Err(error) => {
            CHANGED.lock().expect("changed drafts").insert(resident.clone());
            return Err(error);
        }
    };
    let held = producing.hold(resident).await;
    let Some(Beside { library, scope, holding }) = (match writable(node, resident, ConversationDraft::STREAM, &held).await {
        Ok(beside) => beside,
        Err(error) => {
            CHANGED.lock().expect("changed drafts").insert(resident.clone());
            return Err(error);
        }
    }) else {
        CHANGED.lock().expect("changed drafts").insert(resident.clone());
        return Ok(0);
    };
    let draft = &composer.draft;
    let body = serde_json::to_vec(&serde_json::json!({
        "epoch": draft.epoch,
        "revision": composer.revision,
        "text": draft.text,
        "attachments": draft.attachments,
        "uploads": draft.uploads,
        "files_on": draft.files_on,
    }))
    .map_err(failure)?;
    let signer = node.assets.as_ref().map(|assets| assets.signer()).ok_or_else(|| failure("no asset library"))?;
    let (epoch, revision) = (draft.epoch, composer.revision);
    let written = fastctx_platform::task::blocking(move || {
        let _held = held;
        use sha2::Digest as _;
        let digest: [u8; 32] = sha2::Sha256::digest(&body).into();
        let same = library.item(&scope, ConversationDraft::STREAM, ITEM).map_err(failure)?.is_some_and(|entry| match entry.objects.first() {
            None => entry.body == body,
            Some(object) => object.hash == digest && object.size == body.len() as u64,
        });
        if same {
            return Ok(0);
        }
        // A draft larger than a body holds is its one object (AS16).
        let (value, objects, leases) = if body.len() > BODY_MOST {
            let lease = library.put(&mut body.as_slice(), "draft", fastctx_content::v2::LEASE).map_err(failure)?;
            let said = serde_json::json!({ "in_object": true, "epoch": epoch, "revision": revision, "bytes": body.len() });
            (serde_json::to_vec(&said).map_err(failure)?, vec![lease.object], vec![lease])
        } else {
            (body, Vec::new(), Vec::new())
        };
        Writer::<ConversationDraft>::new(&library, &scope, ConversationDraft::STREAM)
            .map_err(failure)?
            .in_holding(holding)
            .write_all(
                vec![Write { item: ITEM.to_vec(), order: ITEM.to_vec(), value: Some(value), objects }],
                leases,
                false,
                signer,
            )
            .map_err(failure)?;
        Ok::<_, Failure>(1)
    })
    .await;
    match written {
        Ok(written) => {
            LOOKED.lock().expect("looked drafts").insert(resident.clone(), holding);
            Ok(written)
        }
        Err(error) => {
            CHANGED.lock().expect("changed drafts").insert(resident.clone());
            Err(error)
        }
    }
}
