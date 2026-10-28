//! The files the user attached follow the conversation to each machine it
//! runs a script on: before the script starts there, those its scratch on
//! that machine does not hold yet are brought into it, from wherever each is
//! kept, for the account the conversation works for. A file is brought once:
//! what a script then does with its copy is its own (`world-run.md`,
//! 用户附的文件).

use std::collections::{HashMap, HashSet};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use fastctx_net::file_read::Part;
use fastctx_types::access::Acting;
use fastctx_types::attachments::{Attachment, AttachmentSource};
use fastctx_types::file_read::FileRange;
use fastctx_types::run::MissingAttachment;
use fastctx_types::session::attachments::{file_name, place};
use sha2::{Digest as _, Sha256};
use tokio::sync::watch;

use crate::Node;

/// How long a `run` waits for its files before its script starts anyway,
/// the rest still arriving.
pub(crate) const RUN_WAITS: Duration = Duration::from_secs(20);

/// What becomes of one file being brought: nothing yet, or how it ended,
/// with what the model is told when it did not come.
type Bringing = watch::Receiver<Option<Result<(), &'static str>>>;

/// What the model is told of a file not brought. What went wrong in detail
/// goes to the node's log: it may name machines the model is not to know.
const OFFLINE: &str = "the computer it is kept on is offline";
const UNREAD: &str = "it could not be read where it is kept";
const UNPLACED: &str = "it could not be written into the scratch here";

/// The files being brought on this machine, by the directory each goes to,
/// so two scripts starting at once bring a file once.
static BRINGING: LazyLock<Mutex<HashMap<PathBuf, Bringing>>> = LazyLock::new(Default::default);

/// The files a script here was told could not be brought, by the directory
/// each goes to. A later script still tries to bring them, and is not told
/// again of one it was told of.
static TOLD: LazyLock<Mutex<HashSet<PathBuf>>> = LazyLock::new(Default::default);

/// The list a guest room keeps of the files its conversation was given, as
/// its home last sent them, for the `on`s its scripts run from here.
fn listed(room: &fastctx_types::layout::ConversationDir) -> PathBuf {
    room.attachments().join("given.json")
}

/// The files the conversation `owner` was given, as this machine knows
/// them: from its own records when it lives here, else as its home last
/// sent them with a script.
pub(crate) fn known(node: &Node, owner: &fastctx_types::Address) -> Vec<Attachment> {
    let read = if node.is_me(&owner.machine) {
        super::delivered::all(&node.world.conversation(&owner.resident))
    } else {
        match std::fs::read(listed(&node.world.owner(&node.names(), owner))) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(std::io::Error::other),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(error) => Err(error),
        }
    };
    read.unwrap_or_else(|error| {
        node.log.line(format_args!("the files {owner} was given: {error}"));
        Vec::new()
    })
}

/// Brings into the scratch of `owner` here the files of `files` it does not
/// hold yet, for `acting`. Waits for them at most `wait`, or until all are
/// here; says which were not here by then, still arriving or not to be had.
pub(crate) async fn bring(
    node: &Arc<Node>,
    owner: &fastctx_types::Address,
    acting: &Acting,
    files: Vec<Attachment>,
    wait: Option<Duration>,
) -> Vec<MissingAttachment> {
    if files.is_empty() {
        return Vec::new();
    }
    let room = node.world.owner(&node.names(), owner);
    if !node.is_me(&owner.machine)
        && let Err(error) = keep_list(&room, &files)
    {
        node.log.line(format_args!("keeping the files {owner} was given: {error}"));
    }
    let base = room.scratch().join("attachments");
    let mut waiting = Vec::new();
    for attachment in files {
        let directory = base.join(attachment.id.as_str());
        if directory.exists() {
            continue;
        }
        let bringing = {
            let mut bringing = BRINGING.lock().expect("never poisoned");
            // One brought since: it is put in place before it is let go of.
            if directory.exists() {
                continue;
            }
            match bringing.get(&directory) {
                Some(already) => already.clone(),
                None => {
                    let (sender, receiver) = watch::channel(None);
                    bringing.insert(directory.clone(), receiver.clone());
                    let (node, acting, attachment, base) =
                        (node.clone(), acting.clone(), attachment.clone(), base.clone());
                    tokio::spawn(async move {
                        let result = fetch(&node, &acting, &attachment, &base).await.map_err(|(said, why)| {
                            node.log.line(format_args!("bringing {} to {}: {why}", attachment.id, base.display()));
                            said
                        });
                        BRINGING.lock().expect("never poisoned").remove(&base.join(attachment.id.as_str()));
                        sender.send_replace(Some(result));
                    });
                    receiver
                }
            }
        };
        waiting.push((directory, place(&attachment), bringing));
    }
    let all = async {
        for (_, _, bringing) in &mut waiting {
            let _ = bringing.wait_for(Option::is_some).await;
        }
    };
    match wait {
        Some(wait) => {
            let _ = tokio::time::timeout(wait, all).await;
        }
        None => all.await,
    }
    let mut told = TOLD.lock().expect("never poisoned");
    waiting
        .into_iter()
        .filter_map(|(directory, path, bringing)| match *bringing.borrow() {
            Some(Ok(())) => {
                told.remove(&directory);
                None
            }
            Some(Err(said)) => told.insert(directory).then(|| MissingAttachment { path, why: Some(said.to_string()) }),
            None => Some(MissingAttachment { path, why: None }),
        })
        .collect()
}

fn keep_list(room: &fastctx_types::layout::ConversationDir, files: &[Attachment]) -> std::io::Result<()> {
    let path = listed(room);
    let bytes = serde_json::to_vec(files).map_err(std::io::Error::other)?;
    if std::fs::read(&path).is_ok_and(|kept| kept == bytes) {
        return Ok(());
    }
    fastctx_platform::ensure_private_dir(&room.attachments())?;
    fastctx_platform::publish(&path, &bytes, fastctx_platform::Permissions::Private)
}

/// Reads `attachment` from where it is kept, for `acting`, into its own
/// directory under `base`: written beside it first, and put in place whole
/// once its bytes are the ones the user attached. A failure is what the
/// model is told, and what went wrong.
async fn fetch(
    node: &Arc<Node>,
    acting: &Acting,
    attachment: &Attachment,
    base: &Path,
) -> Result<(), (&'static str, String)> {
    let holder = &attachment.owner.machine;
    if !node.is_me(holder)
        && node.links().iter().any(|link| link.view().member(holder).is_some_and(|member| !member.online))
    {
        return Err((OFFLINE, format!("{holder} is offline")));
    }
    let unread = |why: String| (UNREAD, why);
    let unplaced = |why: String| (UNPLACED, why);
    let part = base.join(format!(".{}.part", attachment.id));
    let name = file_name(&attachment.file.name);
    let file = {
        let (base, part, name) = (base.to_path_buf(), part.clone(), name.clone());
        tokio::task::spawn_blocking(move || -> std::io::Result<std::fs::File> {
            match std::fs::remove_dir_all(&part) {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error),
                _ => {}
            }
            fastctx_platform::ensure_private_dir(&base)?;
            fastctx_platform::ensure_private_dir(&part)?;
            std::fs::File::create_new(part.join(name))
        })
        .await
        .map_err(|error| unplaced(error.to_string()))?
        .map_err(|error| unplaced(format!("making its place: {error}")))?
    };
    let mut writing = Writing { file, hash: Sha256::new(), size: 0 };
    let source = AttachmentSource::Upload(attachment.clone());
    let mut parts = crate::file_read::attachment_for(node.clone(), Some(acting.clone()), source, FileRange::All);
    loop {
        match parts.recv().await {
            Some(Ok(Part::Opened(_))) => {}
            Some(Ok(Part::Chunk { offset, bytes })) => {
                if offset != writing.size {
                    return Err(unread(format!("its bytes came out of order, at {offset} after {}", writing.size)));
                }
                writing = tokio::task::spawn_blocking(move || {
                    let mut writing = writing;
                    writing.write(&bytes).map(|()| writing)
                })
                .await
                .map_err(|error| unplaced(error.to_string()))?
                .map_err(|error| unplaced(format!("writing it: {error}")))?;
            }
            Some(Ok(Part::Finished(Ok(_)))) => break,
            Some(Ok(Part::Finished(Err(error)))) => return Err(unread(error.message)),
            Some(Err(failure)) => return Err(unread(failure.message)),
            None => return Err(unread("its reading stopped before its end".into())),
        }
    }
    let Writing { file, hash, size } = writing;
    if size != attachment.file.size || format!("{:x}", hash.finalize()) != attachment.file.sha256 {
        return Err(unread("what was read differs from the file the user attached".into()));
    }
    let directory = base.join(attachment.id.as_str());
    tokio::task::spawn_blocking(move || {
        file.sync_all()?;
        drop(file);
        std::fs::rename(&part, &directory)
    })
    .await
    .map_err(|error| unplaced(error.to_string()))?
    .map_err(|error| unplaced(format!("putting it in place: {error}")))
}

struct Writing {
    file: std::fs::File,
    hash: Sha256,
    size: u64,
}

impl Writing {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        self.file.write_all(bytes)?;
        self.hash.update(bytes);
        self.size += bytes.len() as u64;
        Ok(())
    }
}
