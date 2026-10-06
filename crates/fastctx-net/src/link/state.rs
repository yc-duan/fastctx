//! What a machine keeps on disk about its link to the hub, in
//! `world/net/`.
//!
//! ```text
//! membership.json   which World, under what name, how to reach and trust its hub
//! members.json      what this machine has accepted about the members (`trust::Known`)
//! counter.json      how far the numbers this machine signs with are reserved
//! letters.json      where this machine's letters stand with the hub
//! calls.json        which calls from each member this machine has taken
//! outbox/<seq>      letters not yet held by the hub
//! received/<from>/<id>   letters taken, by sender and id, for good
//! failures/         letters that could not be delivered or opened, and why
//! ```

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use fastctx_platform::{Permissions, ensure_private_dir, publish};
use fastctx_types::MachineName;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::sign::{PublicKey, Signed};
use crate::trust::{CertPin, Known, WorldId};

/// This machine's place in a World.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Membership {
    pub world: WorldId,
    pub name: MachineName,
    /// The root every member's record must lead back to.
    pub founder: PublicKey,
    pub hub: Vec<SocketAddr>,
    pub cert: CertPin,
}

/// The members as last checked, kept so a machine that starts without the
/// hub still knows them.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct MembersFile {
    pub known: Known,
    pub records: Vec<Signed>,
    pub revocations: Vec<Signed>,
    #[serde(default)]
    pub grants: Vec<Signed>,
}

/// Where this machine's letters stand with the hub.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct LettersFile {
    /// This machine's numbering of its own letters.
    pub epoch: u64,
    /// The last of its letters the hub holds.
    pub held: u64,
    /// The hub's numbering of its letters to this machine, and the last of
    /// them this machine took.
    pub taken: Option<(u64, u64)>,
}

/// The paths of `world/net/`.
#[derive(Clone, Debug)]
pub struct NetDir {
    root: PathBuf,
}

impl NetDir {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn path(&self) -> &Path {
        &self.root
    }

    pub fn membership(&self) -> PathBuf {
        self.root.join("membership.json")
    }

    pub fn members(&self) -> PathBuf {
        self.root.join("members.json")
    }

    pub fn counter(&self) -> PathBuf {
        self.root.join("counter.json")
    }

    pub fn letters(&self) -> PathBuf {
        self.root.join("letters.json")
    }

    pub fn calls(&self) -> PathBuf {
        self.root.join("calls.json")
    }

    pub fn outbox(&self) -> PathBuf {
        self.root.join("outbox")
    }

    pub fn received(&self, from: &MachineName) -> PathBuf {
        self.root.join("received").join(from.as_str())
    }

    pub fn failures(&self) -> PathBuf {
        self.root.join("failures")
    }

    /// Makes the directories, private.
    pub fn prepare(&self) -> io::Result<()> {
        for directory in [self.root.clone(), self.outbox(), self.root.join("received"), self.failures()] {
            ensure_private_dir(&directory)?;
        }
        Ok(())
    }
}

/// The numbers a machine signs its messages with. They are reserved on disk
/// a block at a time before use, so a machine that crashes never signs two
/// messages with one number.
pub struct Counter {
    path: PathBuf,
    state: Mutex<(u64, u64)>,
}

const RESERVED_AT_ONCE: u64 = 1000;

#[derive(Serialize, Deserialize)]
struct CounterFile {
    reserved: u64,
}

impl Counter {
    pub fn open(path: PathBuf) -> io::Result<Self> {
        let start = read_or(&path, CounterFile { reserved: 1 })?.reserved;
        let reserved = start + RESERVED_AT_ONCE;
        write(&path, &CounterFile { reserved })?;
        Ok(Self { path, state: Mutex::new((start, reserved)) })
    }

    pub fn next(&self) -> io::Result<u64> {
        let mut state = self.state.lock().expect("never poisoned");
        if state.0 == state.1 {
            let reserved = state.1 + RESERVED_AT_ONCE;
            write(&self.path, &CounterFile { reserved })?;
            state.1 = reserved;
        }
        let n = state.0;
        state.0 += 1;
        Ok(n)
    }
}

/// The calls this machine has taken from each member, so the hub, which
/// passes every call on, cannot pass one again to have it done twice.
/// Calls from one member arrive nearly in the order it numbered them; any
/// number more than `WINDOW` below the highest taken is refused, and every
/// number within it is remembered.
pub struct Taken {
    path: PathBuf,
    by: Mutex<BTreeMap<MachineName, TakenFrom>>,
}

const WINDOW: u64 = 1024;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct TakenFrom {
    highest: u64,
    recent: BTreeSet<u64>,
}

impl Taken {
    pub fn open(path: PathBuf) -> io::Result<Self> {
        let by = read_or(&path, BTreeMap::new())?;
        Ok(Self { path, by: Mutex::new(by) })
    }

    /// Takes call `n` from `from`, unless it was taken before.
    pub fn take(&self, from: &MachineName, n: u64) -> io::Result<Result<(), String>> {
        let mut by = self.by.lock().expect("never poisoned");
        let taken = by.entry(from.clone()).or_default();
        if n + WINDOW <= taken.highest {
            return Ok(Err(format!("call {n} from {from} is older than the last {WINDOW} this machine took from it")));
        }
        if !taken.recent.insert(n) {
            return Ok(Err(format!("call {n} from {from} was taken before")));
        }
        taken.highest = taken.highest.max(n);
        let floor = taken.highest.saturating_sub(WINDOW);
        taken.recent.retain(|&seen| seen > floor);
        write(&self.path, &*by)?;
        Ok(Ok(()))
    }
}

pub fn read_or<T: DeserializeOwned>(path: &Path, default: T) -> io::Result<T> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, format!("{}: {error}", path.display()))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(default),
        Err(error) => Err(error),
    }
}

pub fn read<T: DeserializeOwned>(path: &Path) -> io::Result<T> {
    let bytes = std::fs::read(path)?;
    serde_json::from_slice(&bytes)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, format!("{}: {error}", path.display())))
}

/// Replaces the file at `path`, in a directory that exists.
pub fn write(path: &Path, value: &impl Serialize) -> io::Result<()> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(io::Error::other)?;
    bytes.push(b'\n');
    publish(path, &bytes, Permissions::Private)
}
