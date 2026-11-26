//! What a device keeps of each hub's asset sync (design §6.1, §17), as a
//! node does (`fastctx-node` `assets.rs`): the hub's service key as first
//! met, the signed rotations that lead to it, and the latest chain head the
//! hub's word carried. Kept in `hubs.json` beside the library, written
//! before it counts.

use fastctx_net::ids::HubId;
use fastctx_net::sign::{Domain, PublicKey, heard_word};
use fastctx_platform::Permissions;
use fastctx_types::assets::{Bytes, Rotation, Seen};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io;
use std::path::PathBuf;
use std::sync::Mutex;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Pinned {
    key: Bytes,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    seen: Option<Seen>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    lineage: Vec<Rotation>,
}

/// Rotations kept of a hub's lineage, and how many an open session carries.
const LINEAGE_KEPT: usize = 256;
const LINEAGE_SENT: usize = 64;

/// `lineage` with `rotations`, given oldest first, in the chain's order; the
/// latest `LINEAGE_KEPT` kept. Whether it changed.
fn grow(lineage: &mut Vec<Rotation>, rotations: &[Rotation]) -> bool {
    let before = lineage.clone();
    let mut grown: Vec<Rotation> = lineage.iter().filter(|rotation| !rotations.contains(rotation)).cloned().collect();
    grown.extend(rotations.iter().cloned());
    if grown.len() > LINEAGE_KEPT {
        grown.drain(..grown.len() - LINEAGE_KEPT);
    }
    *lineage = grown;
    *lineage != before
}

/// Whether `word` is the old service key's signature that `new` follows it.
fn rotated(old: &[u8; 32], new: &[u8; 32], word: &[u8]) -> bool {
    let Ok(signature) = <[u8; 64]>::try_from(word) else { return false };
    let mut statement = b"fastctx-hub-key-rotation/1".to_vec();
    statement.extend_from_slice(old);
    statement.extend_from_slice(new);
    PublicKey::from_bytes(*old)
        .verify(Domain::SyncHubKeyRotation, &statement, &fastctx_net::sign::Signature::from_bytes(signature))
        .is_ok()
}

pub(super) struct Pins {
    path: PathBuf,
    hubs: Mutex<BTreeMap<String, Pinned>>,
}

impl Pins {
    pub(super) fn open(path: PathBuf) -> io::Result<Self> {
        let hubs = match fastctx_platform::disk::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, format!("{}: {error}", path.display())))?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => BTreeMap::new(),
            Err(error) => return Err(error),
        };
        Ok(Self { path, hubs: Mutex::new(hubs) })
    }

    fn save(&self, hubs: &BTreeMap<String, Pinned>) -> io::Result<()> {
        let bytes = serde_json::to_vec_pretty(hubs).map_err(io::Error::other)?;
        fastctx_platform::publish(&self.path, &bytes, Permissions::Private)
    }

    /// Writes `change` of the hubs kept, on the disk before in memory, all
    /// under the one lock it was judged under.
    fn change(
        &self,
        hubs: &mut BTreeMap<String, Pinned>,
        change: impl FnOnce(&mut BTreeMap<String, Pinned>),
    ) -> io::Result<()> {
        let mut next = hubs.clone();
        change(&mut next);
        self.save(&next)?;
        *hubs = next;
        Ok(())
    }

    pub(super) fn pinned(&self, hub: &HubId) -> Option<Bytes> {
        self.hubs.lock().expect("hubs").get(hub.as_str()).map(|pinned| pinned.key.clone())
    }

    pub(super) fn lineage(&self, hub: &HubId) -> Vec<Rotation> {
        let hubs = self.hubs.lock().expect("hubs");
        let Some(pinned) = hubs.get(hub.as_str()) else { return Vec::new() };
        pinned.lineage[pinned.lineage.len().saturating_sub(LINEAGE_SENT)..].to_vec()
    }

    pub(super) fn seen(&self, hub: &HubId) -> Option<Seen> {
        self.hubs.lock().expect("hubs").get(hub.as_str()).and_then(|pinned| pinned.seen.clone())
    }

    /// The hub's service key as pinned: `offered` the first time, or the key
    /// the hub's signed rotations lead to from the one pinned. An error
    /// where none leads to it.
    pub(super) fn pin_following(
        &self,
        hub: &HubId,
        offered: &Bytes,
        rotations: &[Rotation],
    ) -> Result<PublicKey, String> {
        let offered_key: [u8; 32] = offered.array().ok_or("a service key is 32 bytes")?;
        let mut hubs = self.hubs.lock().expect("hubs");
        let pinned = hubs.get(hub.as_str()).and_then(|pinned| pinned.key.array());
        let error = |error: io::Error| error.to_string();
        let Some(mut at) = pinned else {
            self.change(&mut hubs, |hubs| {
                let mut lineage = Vec::new();
                grow(&mut lineage, rotations);
                hubs.insert(hub.as_str().to_string(), Pinned { key: offered_key.into(), seen: None, lineage });
            })
            .map_err(error)?;
            return Ok(PublicKey::from_bytes(offered_key));
        };
        if at != offered_key {
            for _ in 0..=rotations.len() {
                let Some(next) = rotations.iter().find_map(|rotation| {
                    let (old, new) = (rotation.old.array()?, rotation.new.array()?);
                    (old == at && rotated(&old, &new, &rotation.word.0)).then_some(new)
                }) else {
                    break;
                };
                at = next;
                if at == offered_key {
                    break;
                }
            }
            if at != offered_key {
                return Err(format!(
                    "the hub {hub} offers another service key than the one this device pinned, and no rotation it signed leads to it"
                ));
            }
        }
        let grows = hubs.get(hub.as_str()).is_some_and(|pinned| {
            pinned.key.array() != Some(offered_key) || grow(&mut pinned.lineage.clone(), rotations)
        });
        if grows {
            self.change(&mut hubs, |hubs| {
                if let Some(pinned) = hubs.get_mut(hub.as_str()) {
                    pinned.key = offered_key.into();
                    grow(&mut pinned.lineage, rotations);
                }
            })
            .map_err(error)?;
        }
        Ok(PublicKey::from_bytes(offered_key))
    }

    /// The hub set aside the history this device saw as it recovered.
    pub(super) fn forget_seen(&self, hub: &HubId) -> io::Result<()> {
        let mut hubs = self.hubs.lock().expect("hubs");
        self.change(&mut hubs, |hubs| {
            if let Some(pinned) = hubs.get_mut(hub.as_str()) {
                pinned.seen = None;
            }
        })
    }

    /// The hub's word on `statement`, if it is `key`'s: the chain head it
    /// carried is kept when it is the latest seen. A head that could not be
    /// kept on the disk counts for nothing.
    pub(super) fn heard(&self, hub: &HubId, key: &PublicKey, statement: &[u8], word: &[u8]) -> bool {
        let Some((seq, head)) = heard_word(key, statement, word) else { return false };
        let mut hubs = self.hubs.lock().expect("hubs");
        let latest = hubs.get(hub.as_str()).map(|pinned| pinned.seen.as_ref().map(|seen| seen.seq));
        match latest {
            None => false,
            Some(Some(seen)) if seen >= seq => true,
            Some(_) => self
                .change(&mut hubs, |hubs| {
                    if let Some(pinned) = hubs.get_mut(hub.as_str()) {
                        pinned.seen = Some(Seen {
                            seq,
                            head: head.into(),
                            statement: statement.to_vec().into(),
                            word: word.to_vec().into(),
                        });
                    }
                })
                .is_ok(),
        }
    }
}
