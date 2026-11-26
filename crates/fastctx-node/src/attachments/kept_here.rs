//! An attachment's original from the asset library here (design §28.27,
//! 3-5): every device that holds a conversation holds its attachments'
//! originals as objects, so one kept on another machine — offline, or far —
//! is read from here. Written out for as long as it is read, under the
//! world's `passing/`, and let go as the reading ends.

use crate::Node;
use fastctx_types::attachments::Attachment;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// An original written out to read, gone as it is dropped.
pub(crate) struct Passing(PathBuf);

impl Passing {
    /// What is written out at `path`, gone as the value is dropped.
    pub(crate) fn new(path: PathBuf) -> Self {
        Self(path)
    }

    pub(crate) fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Passing {
    fn drop(&mut self) {
        let _ = fastctx_platform::disk::remove_file(&self.0);
    }
}

/// The original of `receipt`, from the asset library here, if it holds it
/// whole for the receipt's own conversation, or for a conversation forked
/// from it here that kept it: written out to read.
///
/// Only those conversations' scopes are asked, each as the copy of that
/// conversation where it lives (`timeline::copy::of`): the same name on
/// another machine or network is another conversation. Who reads an
/// attachment was judged against the conversation its receipt names; bytes
/// with the same digest kept for another conversation, perhaps another
/// account's, are not that conversation's to give, however the digest and
/// length came to be known.
pub(crate) async fn original(node: &Arc<Node>, receipt: &Attachment) -> Option<Passing> {
    let assets = node.assets.as_ref()?;
    let file = &receipt.file;
    let hash = hash_of(&file.sha256)?;
    let (library, size) = (assets.library.clone(), file.size);
    let conversation = receipt.conversation().clone();
    let mut asked: Vec<(fastctx_types::ResidentId, fastctx_types::access::Written)> =
        vec![(conversation.resident.clone(), conversation.machine.clone().into())];
    // Forks of it living here keep its attachments in their own scopes.
    if node.is_me(&conversation.machine) {
        asked.extend(
            node.residents
                .everyone()
                .filter(|one| one.about.forked_from.as_ref() == Some(&conversation))
                .map(|one| (one.id, node.machine().into())),
        );
    }
    let folder = node.world.path().join("passing");
    let asking = node.clone();
    fastctx_platform::task::blocking(move || -> Option<Passing> {
        if library.size(&hash).ok()?? != size {
            return None;
        }
        // Only as one logged in here reads it: a conversation held here as
        // such refers to it now. Bytes still on the disk after the scope was
        // let go, or kept for another's, are read by no one (N12).
        let readable = asked.iter().any(|(resident, machine)| {
            crate::timeline::copy::scope_of_conversation(&asking, resident, machine)
                .is_some_and(|scope| library.refers(&scope, &hash).unwrap_or(false))
        });
        if !readable {
            return None;
        }
        fastctx_platform::disk::create_dir_all(&folder).ok()?;
        let name: String = hash.iter().map(|byte| format!("{byte:02x}")).collect();
        let path = folder
            .join(format!("{name}-{}", std::process::id() as u64 ^ jiff::Timestamp::now().as_nanosecond() as u64));
        let passing = Passing(path.clone());
        let mut out = fastctx_platform::disk::create(&path).ok()?;
        let mut at = 0u64;
        while at < size {
            let piece = library.read(&hash, at, 1 << 20).ok()?;
            if piece.is_empty() {
                return None;
            }
            std::io::Write::write_all(&mut out, &piece).ok()?;
            at += piece.len() as u64;
        }
        Some(passing)
    })
    .await
}

/// Whether any original is written out to be read now.
pub(crate) fn passing(node: &Node) -> bool {
    fastctx_platform::disk::read_dir(node.world.path().join("passing"))
        .is_ok_and(|mut entries| entries.next().is_some())
}

/// What a stop left of originals written out to read: none is read any
/// more as the node starts (PP21).
pub(crate) fn clear_left(node: &Node) {
    let folder = node.world.path().join("passing");
    if fastctx_platform::disk::metadata(&folder).is_ok() {
        let _ = fastctx_platform::disk::remove_dir_all(&folder);
    }
}

fn hash_of(sha256: &str) -> Option<[u8; 32]> {
    if sha256.len() != 64 {
        return None;
    }
    let mut hash = [0u8; 32];
    for (at, byte) in hash.iter_mut().enumerate() {
        *byte = u8::from_str_radix(sha256.get(at * 2..at * 2 + 2)?, 16).ok()?;
    }
    Some(hash)
}
