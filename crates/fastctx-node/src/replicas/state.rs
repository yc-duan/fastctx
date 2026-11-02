//! Durable current copies and pending edits. Historical revisions stay at the hub.

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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Pending {
    pub id: String,
    pub base: Option<u64>,
    pub before: String,
    pub text: String,
    pub operation: WriteOp,
    pub author: Author,
    pub offline: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Observed {
    pub version: Option<u64>,
    pub hash: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Copy {
    #[serde(skip)]
    pub epoch: u64,
    pub account: HubAccount,
    pub head: Head,
    pub path: PathBuf,
    pub text: String,
    #[serde(default)]
    pub observed: BTreeMap<String, Observed>,
    #[serde(default)]
    pub pending: Vec<Pending>,
}

impl Copy {
    pub(crate) fn effective(&self) -> &str {
        self.pending.iter().rfind(|pending| pending.offline).map_or(&self.text, |pending| &pending.text)
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
}

impl From<&Copy> for Route {
    fn from(copy: &Copy) -> Self {
        Self { epoch: copy.epoch, account: copy.account.clone(), head: copy.head.clone(), path: copy.path.clone() }
    }
}

pub(crate) type Slot = Arc<tokio::sync::Mutex<Copy>>;

#[derive(Default)]
pub(crate) struct State {
    pub copies: Mutex<BTreeMap<String, Slot>>,
    pub folders: Mutex<BTreeMap<PathBuf, Folder>>,
    pub routes: Mutex<BTreeMap<String, Route>>,
    pub publication: Mutex<()>,
    paths: Mutex<BTreeMap<String, String>>,
    loaded: Mutex<bool>,
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

fn root(world: &fastctx_types::layout::WorldDir) -> PathBuf {
    world.path().join("replicas")
}

pub(crate) fn save(world: &fastctx_types::layout::WorldDir, copy: &Copy) -> io::Result<()> {
    let directory = root(world).join("copies");
    fastctx_platform::ensure_private_dir(&directory)?;
    fastctx_platform::publish(
        &directory.join(format!("{}.json", copy.id())),
        &serde_json::to_vec(copy)?,
        fastctx_platform::Permissions::Private,
    )
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
    match fastctx_platform::disk::remove_file(
        root(world).join("copies").join(format!("{}.json", identity(&copy.account, &copy.head))),
    ) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

impl State {
    pub(crate) fn load(&self, world: &fastctx_types::layout::WorldDir) -> io::Result<()> {
        let mut loaded = self.loaded.lock().expect("never poisoned");
        if *loaded {
            return Ok(());
        }
        let read = |path: &Path| -> io::Result<Vec<Vec<u8>>> {
            let mut bytes = Vec::new();
            match fastctx_platform::disk::read_dir(path) {
                Ok(entries) => {
                    for entry in entries {
                        let path = entry?.path();
                        if path.extension().is_some_and(|extension| extension == "json") {
                            bytes.push(fastctx_platform::disk::read(path)?);
                        }
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
            Ok(bytes)
        };
        let copies = read(&root(world).join("copies"))?
            .into_iter()
            .map(|bytes| serde_json::from_slice::<Copy>(&bytes))
            .collect::<Result<Vec<_>, _>>()?;
        let folders = read(&root(world).join("folders"))?
            .into_iter()
            .map(|bytes| serde_json::from_slice::<Folder>(&bytes))
            .collect::<Result<Vec<_>, _>>()?;
        for copy in copies {
            self.insert(copy);
        }
        for folder in folders {
            self.folders.lock().expect("never poisoned").insert(folder.path.clone(), folder);
        }
        *loaded = true;
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
