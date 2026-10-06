//! The ledger items this machine took from the hub, as their signers signed
//! them, by town id, in `world/net/ledger/<town id>/`:
//!
//! ```text
//! <seq>.json     one item each, sealed as the hub keeps it
//! state.json     the entries not opened yet, and those refused, with why
//! ```
//!
//! An entry is opened into its town's directory as soon as a copy of its key
//! for this machine is here; until then it waits in `unopened`, and a copy
//! sealed for this machine later opens it then.

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::PathBuf;

use fastctx_net::ledger::{Item, TownId};
use fastctx_net::sign::Signed;
use fastctx_platform::{Permissions, ensure_private_dir, publish, publish_new};
use serde::{Deserialize, Serialize};

pub struct Items {
    root: PathBuf,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct State {
    /// Entries kept here that are not opened yet: no copy of their key for
    /// this machine has come.
    pub unopened: BTreeSet<u64>,
    /// Entries that opened and did not check out, with why: they are never
    /// taken into the town.
    pub refused: BTreeMap<u64, String>,
    /// Entries kept unopened because their author did not stand where it
    /// would have to, as this machine knows the members now, with why. They
    /// are judged again whenever the entries are opened: what this machine
    /// knows of the members may be behind.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub doubtful: BTreeMap<u64, String>,
}

impl Items {
    pub fn new(world: &fastctx_types::layout::WorldDir) -> Self {
        Self { root: world.net().join("ledger") }
    }

    fn dir(&self, town: &TownId) -> PathBuf {
        self.root.join(town.as_str())
    }

    /// The towns this machine keeps items of.
    pub fn towns(&self) -> io::Result<Vec<TownId>> {
        let mut towns = Vec::new();
        let entries = match std::fs::read_dir(&self.root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(towns),
            Err(error) => return Err(error),
        };
        for entry in entries {
            if let Some(town) = entry?.file_name().to_str().and_then(|name| TownId::try_from(name.to_string()).ok()) {
                towns.push(town);
            }
        }
        towns.sort();
        Ok(towns)
    }

    /// The number of the latest item kept of `town`; 0 when none.
    pub fn cursor(&self, town: &TownId) -> io::Result<u64> {
        Ok(super::numbered(&self.dir(town))?.last().map(|(seq, _)| *seq).unwrap_or(0))
    }

    /// Keeps item `seq` of `town`; the same item kept twice is kept once.
    pub fn keep(&self, town: &TownId, seq: u64, signed: &Signed) -> io::Result<()> {
        let dir = self.dir(town);
        ensure_private_dir(&self.root)?;
        ensure_private_dir(&dir)?;
        let bytes = serde_json::to_vec(signed).map_err(io::Error::other)?;
        match publish_new(&dir.join(format!("{seq:020}.json")), &bytes, Permissions::Private) {
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(()),
            other => other,
        }
    }

    /// Every item kept of `town`, in order, as its signer signed it; each
    /// was checked when it was taken.
    pub fn all(&self, town: &TownId) -> io::Result<Vec<(u64, Item, Signed)>> {
        let mut items = Vec::new();
        for (seq, path) in super::numbered(&self.dir(town))? {
            let invalid =
                |error: String| io::Error::new(io::ErrorKind::InvalidData, format!("{}: {error}", path.display()));
            let signed: Signed =
                serde_json::from_slice(&std::fs::read(&path)?).map_err(|error| invalid(error.to_string()))?;
            let item: Item = signed.claim().map_err(|error| invalid(error.to_string()))?;
            items.push((seq, item, signed));
        }
        Ok(items)
    }

    pub fn state(&self, town: &TownId) -> io::Result<State> {
        fastctx_net::link::state::read_or(&self.dir(town).join("state.json"), State::default())
    }

    pub fn set_state(&self, town: &TownId, state: &State) -> io::Result<()> {
        let dir = self.dir(town);
        ensure_private_dir(&self.root)?;
        ensure_private_dir(&dir)?;
        let mut bytes = serde_json::to_vec_pretty(state).map_err(io::Error::other)?;
        bytes.push(b'\n');
        publish(&dir.join("state.json"), &bytes, Permissions::Private)
    }
}
