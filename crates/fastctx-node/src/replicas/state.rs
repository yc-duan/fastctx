//! Durable current copies and pending edits. Historical revisions stay at the hub.
//!
//! What identifies a copy stays in memory; its texts do not. Each is kept once
//! on disk under the copy's own folder of bodies, named by its hash, and read
//! when an operation needs it (`bodies`).

use super::bodies::{self, Text};
use fastctx_types::{
    access::HubAccount,
    history::Author,
    replicas::{Document, Head, Scope},
    write::WriteOp,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

/// An edit waiting for the document's hub. Each becomes a revision of its
/// own once the hub has it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Pending {
    pub id: String,
    pub base: Option<u64>,
    /// The text the edit was made on.
    pub before: Text,
    pub text: Text,
    /// The edit as asked, replayed on a newer revision; none for a whole write.
    pub edit: Option<WriteOp>,
    pub author: Author,
    pub offline: bool,
}

/// What a conversation last saw of a copy: the hub revision under it, and
/// the hash of the text it read. `local` when that text held edits made
/// offline, whose revisions the hub numbers only once they reach it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Observed {
    pub version: Option<u64>,
    pub hash: Option<String>,
    #[serde(default)]
    pub local: bool,
}

/// What this computer last wrote at a copy's path: the file still holds it
/// while its length and modification time are these.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Projected {
    pub text: Text,
    pub modified: Option<i128>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Copy {
    #[serde(skip)]
    pub epoch: u64,
    pub account: HubAccount,
    pub head: Head,
    pub path: PathBuf,
    /// The hub's text of `head.version`.
    pub body: Text,
    #[serde(default)]
    pub projected: Option<Projected>,
    #[serde(default)]
    pub observed: BTreeMap<String, Observed>,
    #[serde(default)]
    pub pending: Vec<Pending>,
    /// Paths this copy moved away from, whose projections are still to go.
    #[serde(default)]
    pub left: Vec<PathBuf>,
}

impl Copy {
    /// The text the copy shows: its last edit made offline, else the hub's.
    pub(crate) fn effective(&self) -> &Text {
        self.pending.iter().rfind(|pending| pending.offline).map_or(&self.body, |pending| &pending.text)
    }
    pub(crate) fn id(&self) -> String {
        identity(&self.account, &self.head)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Folder {
    pub account: HubAccount,
    pub scope: Scope,
    pub path: PathBuf,
}

#[derive(Clone)]
pub(crate) struct Route {
    pub epoch: u64,
    pub account: HubAccount,
    pub head: Head,
    pub path: PathBuf,
    pub left: Vec<PathBuf>,
}

impl From<&Copy> for Route {
    fn from(copy: &Copy) -> Self {
        Self {
            epoch: copy.epoch,
            account: copy.account.clone(),
            head: copy.head.clone(),
            path: copy.path.clone(),
            left: copy.left.clone(),
        }
    }
}

pub(crate) type Slot = Arc<tokio::sync::Mutex<Copy>>;

#[derive(Default)]
pub(crate) struct State {
    pub copies: Mutex<BTreeMap<String, Slot>>,
    pub folders: Mutex<BTreeMap<PathBuf, Folder>>,
    pub routes: Mutex<BTreeMap<String, Route>>,
    /// Taken shared by a document's write and alone by what withdraws
    /// documents and folders: a write never lands after its withdrawal.
    pub publication: std::sync::RwLock<()>,
    paths: Mutex<BTreeMap<String, String>>,
    loaded: Mutex<bool>,
    /// Whether the copies kept on disk were read in, asked without waiting.
    read_in: std::sync::atomic::AtomicBool,
}

/// `State::load`, where reading the disk may block; nothing once read.
pub(crate) async fn loaded(node: &Arc<crate::Node>) -> io::Result<()> {
    if node.replicas.state.read_in.load(std::sync::atomic::Ordering::Acquire) {
        return Ok(());
    }
    let node = node.clone();
    fastctx_platform::task::blocking(move || node.replicas.state.load(&node.world)).await
}

pub(crate) fn identity(account: &HubAccount, head: &Head) -> String {
    let document = match &head.document {
        Document::Plan { owner } => serde_json::json!(["plan", head.owner, owner.resident]),
        Document::History { scope, name } => serde_json::json!(["history", scope_key(scope), name]),
    };
    format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_vec(&(account.hub.clone(), head.opened, document)).expect("replica identity serializes")
        )
    )
}

pub(crate) fn scope_key(scope: &Scope) -> String {
    match scope {
        Scope::Project { town, .. } => format!("project:{town}"),
        Scope::Conversation { conversation } => {
            format!("conversation:{}:{}", conversation.account, conversation.address.resident)
        }
    }
}

pub(crate) fn root(world: &fastctx_types::layout::WorldDir) -> PathBuf {
    world.path().join("replicas")
}

/// Keeps a copy's metadata, then lets go of the texts it no longer names.
/// The texts it names were put before.
pub(crate) fn save(world: &fastctx_types::layout::WorldDir, copy: &Copy) -> io::Result<()> {
    let directory = root(world).join("copies");
    fastctx_platform::ensure_private_dir(&directory)?;
    fastctx_platform::publish(
        &directory.join(format!("{}.json", copy.id())),
        &serde_json::to_vec(copy)?,
        fastctx_platform::Permissions::Private,
    )?;
    bodies::tidy(world, copy)
}

pub(crate) fn save_folder(world: &fastctx_types::layout::WorldDir, folder: &Folder) -> io::Result<()> {
    let directory = root(world).join("folders");
    fastctx_platform::ensure_private_dir(&directory)?;
    let id = folder_id(folder);
    fastctx_platform::publish(
        &directory.join(format!("{id}.json")),
        &serde_json::to_vec(folder)?,
        fastctx_platform::Permissions::Private,
    )
}

fn folder_id(folder: &Folder) -> String {
    format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_vec(&(folder.account.hub.clone(), scope_key(&folder.scope)))
                .expect("folder identity serializes")
        )
    )
}

pub(crate) fn delete_folder(world: &fastctx_types::layout::WorldDir, folder: &Folder) -> io::Result<()> {
    super::files::delete(&folder.path)?;
    super::files::delete(&root(world).join("folders").join(format!("{}.json", folder_id(folder))))
}

pub(crate) fn delete(world: &fastctx_types::layout::WorldDir, copy: &Route) -> io::Result<()> {
    super::files::withdraw(&copy.path)?;
    for path in &copy.left {
        super::files::withdraw(path)?;
    }
    let id = identity(&copy.account, &copy.head);
    match fastctx_platform::disk::remove_file(root(world).join("copies").join(format!("{id}.json"))) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    bodies::remove(world, &id)
}

impl State {
    pub(crate) fn load(&self, world: &fastctx_types::layout::WorldDir) -> io::Result<()> {
        let mut loaded = self.loaded.lock().expect("never poisoned");
        if *loaded {
            return Ok(());
        }
        let each = |path: &Path, read: &mut dyn FnMut(PathBuf) -> io::Result<()>| -> io::Result<()> {
            match fastctx_platform::disk::read_dir(path) {
                Ok(entries) => {
                    for entry in entries {
                        let path = entry?.path();
                        if path.extension().is_some_and(|extension| extension == "json") {
                            read(path)?;
                        }
                    }
                    Ok(())
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error),
            }
        };
        each(&root(world).join("copies"), &mut |path| {
            let copy = super::legacy::read(world, &path)?;
            self.insert(copy);
            Ok(())
        })?;
        each(&root(world).join("folders"), &mut |path| {
            let folder: Folder = serde_json::from_slice(&fastctx_platform::disk::read(path)?)?;
            self.folders.lock().expect("never poisoned").insert(folder.path.clone(), folder);
            Ok(())
        })?;
        *loaded = true;
        self.read_in.store(true, std::sync::atomic::Ordering::Release);
        Ok(())
    }

    pub(crate) fn remove(&self, id: &str) {
        self.copies.lock().expect("never poisoned").remove(id);
        self.routes.lock().expect("never poisoned").remove(id);
        self.paths.lock().expect("never poisoned").retain(|_, value| value != id);
    }

    pub(crate) fn at(&self, path: &Path) -> Option<Slot> {
        let id = self.paths.lock().expect("never poisoned").get(&path_key(path)).cloned()?;
        self.get(&id)
    }

    pub(crate) fn slots(&self) -> Vec<Slot> {
        self.copies.lock().expect("never poisoned").values().cloned().collect()
    }
    pub(crate) fn get(&self, id: &str) -> Option<Slot> {
        self.copies.lock().expect("never poisoned").get(id).cloned()
    }
    pub(crate) fn insert(&self, mut copy: Copy) -> Slot {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let id = copy.id();
        let mut copies = self.copies.lock().expect("never poisoned");
        if let Some(slot) = copies.get(&id) {
            return slot.clone();
        }
        copy.epoch = NEXT.fetch_add(1, Ordering::Relaxed);
        self.paths.lock().expect("never poisoned").insert(path_key(&copy.path), id.clone());
        self.routes.lock().expect("never poisoned").insert(id.clone(), Route::from(&copy));
        let slot = Arc::new(tokio::sync::Mutex::new(copy));
        copies.insert(id, slot.clone());
        slot
    }

    pub(crate) fn relocated(&self, copy: &Copy) {
        let id = copy.id();
        self.paths.lock().expect("never poisoned").retain(|_, value| value != &id);
        self.paths.lock().expect("never poisoned").insert(path_key(&copy.path), id);
    }
}

pub(crate) fn path_key(path: &Path) -> String {
    let mut text = path.to_string_lossy().replace('\\', "/");
    if let Some(native) = text.strip_prefix("//?/") {
        text = native.into();
    }
    if cfg!(windows) {
        text.make_ascii_lowercase();
    }
    text
}
