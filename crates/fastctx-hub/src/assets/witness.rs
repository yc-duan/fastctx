//! The hub's chain head kept outside its directory (design §17, item 2): a
//! small file in the service account's state, in no backup of the hub,
//! holding the latest (`hub_seq`, `hub_head`) and, for each owner library,
//! the heads its latest checkpoint wrote. A control fact reaches it, and
//! the disk, after its commit and before anyone is answered or told; a
//! checkpoint after its commit, at most a second late. An ordinary crash
//! leaves it older than the hub's databases, never newer, so a file newer
//! than them on opening means the hub's directory was put back.
//!
//! It only moves forward. Once the hub finds it was put back, the file is
//! left as it is, the evidence of the history that was lost.

use fastctx_content::v2::Head;
use fastctx_platform::Permissions;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

/// What the file holds.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Kept {
    pub seq: u64,
    pub head: String,
    /// For each owner library by its key, the heads its latest checkpoint
    /// wrote.
    pub owners: BTreeMap<String, Vec<KeptHead>>,
}

/// One scope's head as kept.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct KeptHead {
    pub scope: String,
    pub change: u64,
    pub prefix: String,
    pub parked: u64,
    pub accumulated: String,
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub(crate) fn unhex(text: &str) -> Option<[u8; 32]> {
    if text.len() != 64 {
        return None;
    }
    let mut bytes = [0u8; 32];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(text.get(index * 2..index * 2 + 2)?, 16).ok()?;
    }
    Some(bytes)
}

impl KeptHead {
    fn of(head: &Head) -> Self {
        Self {
            scope: head.scope.clone(),
            change: head.change,
            prefix: hex(&head.prefix),
            parked: head.parked,
            accumulated: hex(&head.accumulated),
        }
    }

    pub(crate) fn head(&self) -> Option<Head> {
        Some(Head {
            scope: self.scope.clone(),
            change: self.change,
            prefix: unhex(&self.prefix)?,
            parked: self.parked,
            accumulated: unhex(&self.accumulated)?,
        })
    }
}

/// The file and what it holds now.
pub(crate) struct Witness {
    path: PathBuf,
    kept: Mutex<Kept>,
    frozen: AtomicBool,
}

/// Where the file of the hub `id` kept in `dir` lives: `FASTCTX_HUB_WITNESS_DIR`
/// when set; beside a hub kept in the system's temporary directory (a
/// drill's); otherwise the service account's state directory.
fn location(dir: &Path, id: &str) -> PathBuf {
    let base = if let Some(set) = std::env::var_os("FASTCTX_HUB_WITNESS_DIR") {
        PathBuf::from(set)
    } else if dir.starts_with(std::env::temp_dir()) {
        dir.parent().unwrap_or(dir).join("hub-witness")
    } else if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(|| dir.to_path_buf()).join("fastctx").join("hub-witness")
    } else {
        std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local").join("state")))
            .unwrap_or_else(|| dir.to_path_buf())
            .join("fastctx")
            .join("hub-witness")
    };
    base.join(format!("{id}.json"))
}

impl Witness {
    /// The witness of the hub `id` kept in `dir`, as last written.
    pub(crate) fn open(dir: &Path, id: &str) -> io::Result<Self> {
        let path = location(dir, id);
        let kept = match fastctx_platform::disk::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, format!("{}: {error}", path.display())))?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => Kept::default(),
            Err(error) => return Err(error),
        };
        Ok(Self { path, kept: Mutex::new(kept), frozen: AtomicBool::new(false) })
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn kept(&self) -> Kept {
        self.kept.lock().expect("witness").clone()
    }

    /// The chain head the file holds: what the hub's words are bound to.
    #[allow(dead_code)] // read by the signer once the asset wire lands (3-1)
    pub(crate) fn current(&self) -> (u64, [u8; 32]) {
        let kept = self.kept.lock().expect("witness");
        (kept.seq, unhex(&kept.head).unwrap_or([0; 32]))
    }

    /// Leaves the file as it is from now on.
    pub(crate) fn freeze(&self) {
        self.frozen.store(true, Ordering::SeqCst);
    }

    /// The chain reached (`seq`, `head`), and these owner libraries were
    /// checkpointed at these heads: written and on the disk before it
    /// returns. An older head than the file's is left out.
    pub(crate) fn advanced(&self, seq: u64, head: &[u8; 32], owners: Vec<(String, Vec<Head>)>) -> io::Result<()> {
        if self.frozen.load(Ordering::SeqCst) {
            return Ok(());
        }
        let mut kept = self.kept.lock().expect("witness");
        if seq <= kept.seq && owners.is_empty() {
            return Ok(());
        }
        let mut next = kept.clone();
        if seq > next.seq {
            next.seq = seq;
            next.head = hex(head);
        }
        for (owner, heads) in owners {
            next.owners.insert(owner, heads.iter().map(KeptHead::of).collect());
        }
        if let Some(parent) = self.path.parent() {
            fastctx_platform::ensure_private_dir(parent)?;
        }
        let bytes = serde_json::to_vec_pretty(&next).map_err(io::Error::other)?;
        fastctx_platform::publish(&self.path, &bytes, Permissions::Private)?;
        *kept = next;
        Ok(())
    }
}
