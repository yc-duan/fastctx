//! What this device keeps of the conversations it has viewed, on its own
//! disk (`world-gui.md`, 对话: every device that views a conversation keeps
//! it, and it is still there after a restart). The machine a conversation
//! lives on stays its source; this is a copy, brought up to date each time
//! the conversation is opened, so that opening it, scrolling and switching
//! back draw from here at once and never wait for the network.
//!
//! ```text
//! <root>/conversations/<address key>/index.json      which segments, which version
//! <root>/conversations/<address key>/<segment>.zst   one segment of the history
//! <root>/conversations/<address key>/.lock           held while one is written
//! <root>/pictures/<picture key>-<size>.thumb         a picture made small, by its bytes
//! <root>/originals/<picture key>/<name>              a picture or file as it was sent
//! ```
//!
//! Pictures are kept by their bytes where those are known, so a picture
//! seen in two conversations is fetched once. Names that come from other
//! machines never name a file here: an address and a picture's source are
//! hashed into the names above.

mod conversation;
pub(crate) mod pictures;

use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use fastctx_types::Address;
use serde::Serialize;
use sha2::{Digest as _, Sha256};

pub(crate) use conversation::{Entry, Index};
use pictures::Pictures;

/// This device's cache of the conversations it viewed.
pub struct Cache {
    root: PathBuf,
    pictures: Mutex<Pictures>,
}

/// How much the cache holds.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Usage {
    /// Every byte of it.
    pub bytes: u64,
    /// Pictures and files kept by their bytes, shared by conversations.
    pub pictures: u64,
    pub conversations: Vec<Kept>,
}

/// One conversation's history as this device keeps it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Kept {
    pub address: Address,
    pub bytes: u64,
    /// When this device last brought it up to date.
    pub updated: jiff::Timestamp,
}

impl Cache {
    /// The cache of the World kept at `fastctx_root` on a computer, which
    /// its graphical interface and its terminal one share.
    pub fn under(fastctx_root: &Path) -> io::Result<Arc<Self>> {
        Self::open(fastctx_root.join("cache").join("viewed"))
    }

    /// The cache under `root`, made on first use.
    pub fn open(root: PathBuf) -> io::Result<Arc<Self>> {
        fastctx_platform::ensure_private_dir(&root)?;
        Ok(Arc::new(Self { root, pictures: Mutex::new(Pictures::default()) }))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn entry(&self, address: &Address) -> Entry {
        Entry::new(self.conversations().join(key(&format!("{}\n{}", address.machine, address.resident))))
    }

    fn conversations(&self) -> PathBuf {
        self.root.join("conversations")
    }

    /// Learns what the pictures `events` show are, so they are kept by
    /// their bytes; returns their keys.
    pub(crate) fn learn(&self, events: &[fastctx_types::session::SessionEvent]) -> Vec<String> {
        self.pictures.lock().expect("never poisoned").learn(events)
    }

    /// What a picture from `source` is kept by.
    pub(crate) fn picture_key(&self, source: &fastctx_types::attachments::AttachmentSource) -> String {
        self.pictures.lock().expect("never poisoned").key(source)
    }

    /// What a picture from `source` is kept by when its bytes are not those
    /// it was named by.
    pub(crate) fn source_key(&self, source: &fastctx_types::attachments::AttachmentSource) -> String {
        pictures::source_key(source)
    }

    /// Whether the conversation at `address` is kept here.
    pub fn holds(&self, address: &Address) -> io::Result<bool> {
        Ok(self.entry(address).index()?.is_some())
    }

    /// How much is kept, conversation by conversation.
    pub fn usage(&self) -> io::Result<Usage> {
        let mut usage = Usage::default();
        for name in ["pictures", "originals"] {
            usage.pictures += size_of(&self.root.join(name))?;
        }
        for directory in directories(&self.conversations())? {
            let entry = Entry::new(directory.clone());
            let Some(index) = entry.index()? else { continue };
            usage.conversations.push(Kept {
                address: index.address,
                bytes: size_of(&directory)?,
                updated: index.updated,
            });
        }
        usage.conversations.sort_by(|a, b| b.bytes.cmp(&a.bytes));
        usage.bytes = size_of(&self.root)?;
        Ok(usage)
    }

    /// Lets go of everything kept; what is open draws again from the
    /// machines its conversations live on.
    pub fn clear(&self) -> io::Result<()> {
        for name in ["conversations", "pictures", "originals"] {
            match std::fs::remove_dir_all(self.root.join(name)) {
                Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
                _ => {}
            }
        }
        self.pictures.lock().expect("never poisoned").clear();
        Ok(())
    }

    /// Lets go of one conversation, as when it is removed or deleted, and of
    /// the pictures no other conversation kept here shows.
    pub fn forget(&self, address: &Address) -> io::Result<()> {
        let entry = self.entry(address);
        let gone: HashSet<String> = match entry.index()? {
            Some(index) => index.pictures.into_iter().collect(),
            None => return Ok(()),
        };
        entry.remove()?;
        let mut shown = HashSet::new();
        for directory in directories(&self.conversations())? {
            if let Some(index) = Entry::new(directory).index()? {
                shown.extend(index.pictures);
            }
        }
        pictures::forget(&self.root, &gone.difference(&shown).cloned().collect())
    }
}

/// A name for a file here, made from text that may come from elsewhere.
pub(crate) fn key(text: &str) -> String {
    Sha256::digest(text.as_bytes())[..16].iter().map(|byte| format!("{byte:02x}")).collect()
}

fn directories(path: &Path) -> io::Result<Vec<PathBuf>> {
    let entries = match std::fs::read_dir(path) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut found = Vec::new();
    for entry in entries {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            found.push(entry.path());
        }
    }
    Ok(found)
}

/// The bytes of every file under `path`.
fn size_of(path: &Path) -> io::Result<u64> {
    let entries = match std::fs::read_dir(path) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error),
    };
    let mut bytes = 0;
    for entry in entries {
        let entry = entry?;
        let kind = entry.file_type()?;
        bytes += if kind.is_dir() { size_of(&entry.path())? } else { entry.metadata()?.len() };
    }
    Ok(bytes)
}
