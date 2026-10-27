//! What a machine keeps on disk about its link to one hub, in
//! `world/net/<hub id>/`.
//!
//! ```text
//! membership.json   which hub, under what name, how to reach and trust it
//! view.json         this machine's view of the hub's directory, as last given
//! counter.json      how far the numbers this machine sends with are reserved
//! letters.json      where this machine's letters stand with the hub
//! outbox/<seq>      letters not yet held by the hub
//! received/<from>/<id>   legacy receipts, removed after database migration
//! node.sqlite3      standalone device receipts; nodes share world/node.sqlite3
//! failures/         letters that could not be delivered or opened, and why
//! ended.json        that the hub ended this machine's place, and why
//! ```

use std::io;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use fastctx_platform::{Permissions, ensure_private_dir, publish};
use fastctx_types::{HubName, MachineName};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::directory::Kind;
use crate::ids::{CertPin, HubId};

/// This machine's place on a hub.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Membership {
    pub id: HubId,
    pub hub_name: HubName,
    pub name: MachineName,
    /// The hub's own addresses; the machines that forward to the hub when
    /// this machine's own line is down come from its view.
    pub addresses: Vec<SocketAddr>,
    pub cert: CertPin,
    pub kind: Kind,
    /// The relays the code it joined with named: its ways to the hub until
    /// its first view names them (`Inner::upstreams`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub relays: Vec<SocketAddr>,
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
    database: Option<fastctx_storage::Database>,
    authority: String,
}

impl NetDir {
    pub fn new(root: PathBuf) -> Self {
        Self { root, database: None, authority: String::new() }
    }

    /// Shares the application's single database with this membership. Standalone
    /// device links otherwise keep a private database beside their membership.
    pub fn with_database(mut self, database: fastctx_storage::Database) -> Self {
        self.database = Some(database);
        self
    }

    pub(super) fn open_database(&mut self, hub: &HubId) -> io::Result<()> {
        self.authority = hub.as_str().to_owned();
        if self.database.is_none() {
            self.database = Some(
                fastctx_storage::Database::open(&self.root.join("node.sqlite3"), "").map_err(io::Error::other)?,
            );
        }
        Ok(())
    }

    pub(super) fn database(&self) -> &fastctx_storage::Database {
        self.database.as_ref().expect("link opens its database before receiving")
    }

    pub(super) fn authority(&self) -> &str {
        &self.authority
    }

    pub fn path(&self) -> &Path {
        &self.root
    }

    pub fn membership(&self) -> PathBuf {
        self.root.join("membership.json")
    }

    pub fn view(&self) -> PathBuf {
        self.root.join("view.json")
    }

    pub fn counter(&self) -> PathBuf {
        self.root.join("counter.json")
    }

    pub fn letters(&self) -> PathBuf {
        self.root.join("letters.json")
    }

    pub fn outbox(&self) -> PathBuf {
        self.root.join("outbox")
    }

    pub fn received(&self, from: &MachineName) -> io::Result<PathBuf> {
        from.entry_in(&self.root.join("received"))
    }

    pub fn failures(&self) -> PathBuf {
        self.root.join("failures")
    }

    /// Kept once the hub ended this machine's place on it.
    pub fn ended(&self) -> PathBuf {
        self.root.join("ended.json")
    }

    /// Makes the directories, private.
    pub fn prepare(&self) -> io::Result<()> {
        for directory in [self.root.clone(), self.outbox(), self.root.join("received"), self.failures()] {
            ensure_private_dir(&directory)?;
        }
        Ok(())
    }
}

/// The hub ended this machine's place on it: it was removed, or its network
/// was dissolved. Kept beside the membership, so what reads the link from
/// disk knows the place is over without asking the hub.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Ended {
    pub at: jiff::Timestamp,
    pub reason: String,
    /// The network was dissolved, not only this machine taken off it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub dissolved: bool,
    /// What its remover asked it to do then.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub then: Option<fastctx_types::uninstall::Then>,
}

/// The network this World is a managed machine on and its owner there, as
/// the memberships under `net` keep them, while its place has not ended
/// (`world-network.md`, 受管机器).
pub fn managed_on(net: &Path) -> io::Result<Option<(HubName, fastctx_types::AccountName)>> {
    let entries = match std::fs::read_dir(net) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let dir = NetDir::new(entry?.path());
        if !dir.membership().is_file() || dir.ended().try_exists()? || older_view(&dir.view()) {
            continue;
        }
        let membership: Membership = read(&dir.membership())?;
        let Some(view) = read_or::<Option<crate::directory::View>>(&dir.view(), None)? else { continue };
        if let Some(me) = view.me().filter(|me| me.managed) {
            return Ok(Some((membership.hub_name, me.home.clone().unwrap_or_else(|| "?".parse().expect("a name")))));
        }
    }
    Ok(None)
}

/// The numbers a machine sends its messages under. They are reserved on
/// disk a block at a time before use, so a machine that crashes never sends
/// two calls under one number and takes one's answer for the other's.
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

/// Whether the view at `path` was given by a hub from before accounts had
/// two levels and machines were shared and their agents assisted others: it kept
/// `grants` and `trust`, and accounts without a level. Its words meant
/// other things then, and the cut a member holds cannot be read into
/// today's shape the way the hub reads its whole directory, so it is
/// dropped and the hub gives the view again.
pub fn older_view(path: &Path) -> bool {
    let Ok(bytes) = std::fs::read(path) else { return false };
    let Ok(serde_json::Value::Object(view)) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return false;
    };
    view.contains_key("grants")
        || view.contains_key("trust")
        || view
            .get("accounts")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|accounts| accounts.iter().any(|account| account.get("level").is_none()))
}

/// Replaces the file at `path`, in a directory that exists.
pub fn write(path: &Path, value: &impl Serialize) -> io::Result<()> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(io::Error::other)?;
    bytes.push(b'\n');
    publish(path, &bytes, Permissions::Private)
}
