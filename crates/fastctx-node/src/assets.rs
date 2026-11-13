//! The second version's asset sync on this machine (design §6.5, §6.6, §7,
//! §17): its asset library under `world/assets/`, what it keeps of each
//! hub's asset sync — the service key it pinned when it first met the hub on
//! an authenticated link, and the latest chain head the hub's word carried —
//! and one driver per hub (`session`).
//!
//! Frames this machine orders are signed with its device key, under the
//! domain frames are signed in. Facts and receipts count only under the
//! pinned key: a hub whose key changed is not trusted without a rotation
//! (design §6.1).

mod apart;
pub(crate) mod membership;
mod objects;
pub(crate) mod release;
mod session;
pub(crate) mod standing;
pub(crate) mod takeover;
pub(crate) mod upkeep;

pub(crate) use session::{Proposal, pushed, shared_elsewhere, wake};

use fastctx_content::v2::{HubVerifier, Library, Signer};
use fastctx_net::sign::{Domain, PublicKey, SecretKey, heard_word};
use fastctx_platform::Permissions;
use fastctx_types::access::HubId;
use fastctx_types::assets::{Bytes, Seen};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;

/// What this machine keeps of one hub's asset sync.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Pinned {
    /// The hub's service key, as first met.
    key: Bytes,
    /// The latest chain head the hub's word carried, with that word.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    seen: Option<Seen>,
    /// The hub's signed rotations as it gave them, which lead to the key
    /// pinned: what shows a hub put back to before a rotation that the key
    /// is its own successor (design §6.1).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    lineage: Vec<fastctx_types::assets::Rotation>,
}

/// Rotations kept of a hub's lineage, the latest; and how many of them an
/// open session carries: a hub put back to before that many rotations brings
/// back a keyring export.
const LINEAGE_KEPT: usize = 256;
const LINEAGE_SENT: usize = 64;

/// `lineage` with `rotations`, which the hub gives oldest first, in the
/// order of the chain: those it holds that the hub no longer gives are the
/// older, before the hub's (BB03); the latest `LINEAGE_KEPT` kept. Whether
/// it changed.
fn grow(lineage: &mut Vec<fastctx_types::assets::Rotation>, rotations: &[fastctx_types::assets::Rotation]) -> bool {
    let before = lineage.clone();
    let mut grown: Vec<fastctx_types::assets::Rotation> =
        lineage.iter().filter(|rotation| !rotations.contains(rotation)).cloned().collect();
    grown.extend(rotations.iter().cloned());
    if grown.len() > LINEAGE_KEPT {
        grown.drain(..grown.len() - LINEAGE_KEPT);
    }
    *lineage = grown;
    *lineage != before
}

/// Each object a hub did not give, when it last did not and how many times
/// in a row.
pub(crate) type Unreached = HashMap<(HubId, [u8; 32]), (std::time::Instant, u32)>;

pub(crate) struct Assets {
    pub(crate) library: Library,
    device: Arc<SecretKey>,
    path: PathBuf,
    hubs: Mutex<BTreeMap<String, Pinned>>,
    /// Session numbers: unique among this library's sessions, across
    /// restarts too (they start from the clock).
    sessions: AtomicU64,
    /// Each hub's driver, by the wake that brings it round again.
    pub(crate) drivers: Mutex<HashMap<HubId, Arc<Notify>>>,
    /// Each hub's open session and the service key it is under: what a
    /// checkpoint the hub tells of unasked is taken in.
    pub(crate) current: Mutex<HashMap<HubId, (u64, PublicKey)>>,
    /// Objects wanted that no hub gave lately, and when it last did not:
    /// asked again only after a while, so others are not held up.
    pub(crate) unreached: Mutex<Unreached>,
    /// The holding revision this machine last told each hub it cleared at.
    pub(crate) cleared: Mutex<HashMap<HubId, (u64, Vec<String>)>>,
    /// How far each hub's scopes this machine may hold were listed, and when
    /// they were last listed whole (`session::discover`).
    pub(crate) listed: Mutex<HashMap<HubId, Listed>>,
    /// Counted up each time a listing is forgotten: a listing that began
    /// before is not kept (CC07).
    pub(crate) forgotten: std::sync::atomic::AtomicU64,
    /// Streams to be ordered here from now on, by scope (`takeover`).
    pub(crate) takeovers: Mutex<std::collections::BTreeSet<(String, String)>>,
    /// The networks this machine is on, each under its term, as the library
    /// has them (`membership`).
    pub(crate) networks: Mutex<HashMap<HubId, fastctx_content::v2::Network>>,
    /// What brings the upkeep round (`upkeep`).
    pub(crate) upkeep: Arc<Notify>,
    /// When each hub's scopes were last looked at whole (`session`).
    pub(crate) looked: Mutex<HashMap<HubId, std::time::Instant>>,
    /// The scopes committed to here whose upload has not gone through yet.
    pub(crate) uploads: Mutex<std::collections::BTreeSet<String>>,
    /// The scopes a pass did not go through whole — an upload, a pull, a
    /// check or a giving back failed — looked at by themselves again until
    /// one does (HH03, HH06).
    pub(crate) relook: Mutex<std::collections::BTreeSet<String>>,
    /// The hubs whose scopes' writes not yet uploaded were looked for on
    /// the disk since the node started (HH03).
    pub(crate) seeded: Mutex<std::collections::BTreeSet<HubId>>,
    /// The bytes taken of each hub's owner library since this machine last
    /// stood anew in it, what it took lately, and what it last told each
    /// hub of where it stands (`standing`).
    pub(crate) taken: Mutex<HashMap<(String, String), (String, u64)>>,
    pub(crate) received: Mutex<standing::Received>,
    pub(crate) reported: Mutex<HashMap<HubId, (fastctx_types::sync_status::SyncReport, std::time::Instant)>>,
    /// The hubs a pass of which left a stream to go on with at once (H05).
    pub(crate) more: Mutex<std::collections::BTreeSet<HubId>>,
    /// Streams another device orders that came here with no release for
    /// this machine, told of once (`takeover`).
    pub(crate) unreleased: Mutex<std::collections::BTreeSet<(String, String)>>,
    /// Scopes the account that opened them adopted here (`takeover`).
    pub(crate) adopting: Mutex<std::collections::BTreeSet<String>>,
}

/// How far a hub's scopes this machine may hold were listed.
#[derive(Clone)]
pub(crate) struct Listed {
    /// The last scope the listing looked at.
    pub(crate) through: Option<fastctx_types::assets::HoldingsAt>,
    /// When they were last listed whole.
    pub(crate) whole: std::time::Instant,
    /// What decided what this machine may hold then, as the hub said.
    pub(crate) basis: String,
}

impl Assets {
    /// Forgets how far `hub`'s scopes were listed: the next pass lists all,
    /// and a listing under way when it was forgotten is not kept (CC07).
    pub(crate) fn forget_listing(&self, hub: &HubId) {
        let mut listed = self.listed.lock().expect("listed");
        // What was taken of a scope held no more counts against what is left
        // of the others no longer: the hub reckons more left, never less (H08).
        // The era moves under the same lock a taking checks it under (LL01).
        let mut taken = self.taken.lock().expect("taken");
        self.forgotten.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        taken.retain(|(of, _), _| of != hub.as_str());
        drop(taken);
        listed.remove(hub);
    }

    /// Opens the asset side in `dir` for the machine whose key is `device`.
    /// Call on a thread that may block.
    pub(crate) fn open(dir: PathBuf, device: SecretKey) -> io::Result<Self> {
        fastctx_platform::ensure_private_dir(&dir)?;
        let (library, _) = Library::open(&dir).map_err(|error| io::Error::other(error.to_string()))?;
        // The kinds this machine knows: records of others wait in their
        // frames, out of the index, until a release that knows them, which
        // takes them in now (§2.3).
        let kinds: Arc<dyn fastctx_content::v2::Authorizer> =
            Arc::new(fastctx_content::v2::Kinds::new().register::<fastctx_content::v2::Timeline>().register::<fastctx_content::v2::ConversationMeta>().register::<fastctx_content::v2::ConversationWords>());
        library.know(kinds.clone());
        library.reconsider(kinds, true).map_err(|error| io::Error::other(error.to_string()))?;
        let path = dir.join("hubs.json");
        let hubs = match fastctx_platform::disk::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, format!("{}: {error}", path.display())))?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => BTreeMap::new(),
            Err(error) => return Err(error),
        };
        let start = (jiff::Timestamp::now().as_millisecond().max(0) as u64) << 12;
        Ok(Self {
            library,
            device: Arc::new(device),
            path,
            hubs: Mutex::new(hubs),
            sessions: AtomicU64::new(start),
            drivers: Mutex::new(HashMap::new()),
            current: Mutex::new(HashMap::new()),
            unreached: Mutex::new(HashMap::new()),
            cleared: Mutex::new(HashMap::new()),
            listed: Mutex::new(HashMap::new()),
            forgotten: std::sync::atomic::AtomicU64::new(0),
            takeovers: Mutex::new(std::collections::BTreeSet::new()),
            networks: Mutex::new(HashMap::new()),
            upkeep: Arc::new(Notify::new()),
            looked: Mutex::new(HashMap::new()),
            uploads: Mutex::new(std::collections::BTreeSet::new()),
            relook: Mutex::new(std::collections::BTreeSet::new()),
            seeded: Mutex::new(std::collections::BTreeSet::new()),
            taken: Mutex::new(HashMap::new()),
            received: Mutex::new(standing::Received::default()),
            reported: Mutex::new(HashMap::new()),
            more: Mutex::new(std::collections::BTreeSet::new()),
            unreleased: Mutex::new(std::collections::BTreeSet::new()),
            adopting: Mutex::new(std::collections::BTreeSet::new()),
        })
    }

    /// The asset library of `world` while its node is stopped, for what is
    /// done to it then: a conversation exported (`release`).
    pub(crate) fn open_stopped(world: &fastctx_types::layout::WorldDir) -> io::Result<Library> {
        Library::open(&world.assets()).map(|(library, _)| library).map_err(|error| io::Error::other(error.to_string()))
    }

    /// The directory of the conversation `resident` lives in here.
    pub(crate) fn conversation_dir(&self, resident: &str) -> std::path::PathBuf {
        // `path` is the hubs file in the asset directory, in the world's.
        let world = self.path.parent().and_then(std::path::Path::parent).unwrap_or(&self.path);
        world.join("conversations").join(resident)
    }

    pub(crate) fn session(&self) -> u64 {
        self.sessions.fetch_add(1, Ordering::SeqCst)
    }

    pub(crate) fn signer(&self) -> Arc<dyn Signer> {
        Arc::new(Device(self.device.clone()))
    }

    pub(crate) fn device(&self) -> [u8; 32] {
        *self.device.public().as_bytes()
    }

    /// This machine's word, under its key, that it cleared at `revision`
    /// (design §6.3).
    pub(crate) fn sign_cleared(&self, machine: &str, revision: u64) -> Vec<u8> {
        let statement = fastctx_types::assets::cleared_statement(machine, revision);
        self.device.sign(Domain::ShareCleared, &statement).to_bytes().to_vec()
    }

    fn save(&self, hubs: &BTreeMap<String, Pinned>) -> io::Result<()> {
        let bytes = serde_json::to_vec_pretty(hubs).map_err(io::Error::other)?;
        fastctx_platform::publish(&self.path, &bytes, Permissions::Private)
    }

    /// The hub's service key as pinned, pinning `offered` the first time,
    /// with the rotations the hub gave as its lineage. An error where the hub
    /// offers another key than the one pinned.
    pub(crate) fn pin(&self, hub: &HubId, offered: &Bytes, rotations: &[fastctx_types::assets::Rotation]) -> Result<PublicKey, String> {
        let offered: [u8; 32] = offered.array().ok_or("a service key is 32 bytes")?;
        let mut hubs = self.hubs.lock().expect("hubs");
        match hubs.get(hub.as_str()) {
            Some(pinned) if pinned.key.array() == Some(offered) => {
                let mut next = hubs.clone();
                if let Some(pinned) = next.get_mut(hub.as_str())
                    && grow(&mut pinned.lineage, rotations)
                {
                    self.save(&next).map_err(|error| error.to_string())?;
                    *hubs = next;
                }
                Ok(PublicKey::from_bytes(offered))
            }
            Some(_) => Err(format!("the hub {hub} offers another service key than the one this machine pinned")),
            None => {
                // Pinned once it is on the disk: a key kept only in memory
                // would be met as new again after a restart.
                let mut next = hubs.clone();
                // Kept within its bound from the first (AA13).
                let mut lineage = Vec::new();
                grow(&mut lineage, rotations);
                next.insert(hub.as_str().to_string(), Pinned { key: offered.into(), seen: None, lineage });
                self.save(&next).map_err(|error| error.to_string())?;
                *hubs = next;
                Ok(PublicKey::from_bytes(offered))
            }
        }
    }

    /// The rotations that lead to the key pinned for `hub`, as it gave them.
    pub(crate) fn lineage(&self, hub: &HubId) -> Vec<fastctx_types::assets::Rotation> {
        let hubs = self.hubs.lock().expect("hubs");
        let Some(pinned) = hubs.get(hub.as_str()) else { return Vec::new() };
        pinned.lineage[pinned.lineage.len().saturating_sub(LINEAGE_SENT)..].to_vec()
    }

    /// The service key pinned for `hub`, if one is.
    pub(crate) fn pinned(&self, hub: &HubId) -> Option<Bytes> {
        self.hubs.lock().expect("hubs").get(hub.as_str()).map(|pinned| pinned.key.clone())
    }

    /// The hub's service key as pinned, pinning `offered` the first time, or
    /// following the hub's signed rotations from the key pinned to it (design
    /// §6.1). An error where no rotation leads from the one pinned.
    pub(crate) fn pin_following(&self, hub: &HubId, offered: &Bytes, rotations: &[fastctx_types::assets::Rotation]) -> Result<PublicKey, String> {
        let offered_key: [u8; 32] = offered.array().ok_or("a service key is 32 bytes")?;
        let pinned = self.hubs.lock().expect("hubs").get(hub.as_str()).and_then(|pinned| pinned.key.array());
        let Some(mut at) = pinned else { return self.pin(hub, offered, rotations) };
        if at == offered_key {
            return self.pin(hub, offered, rotations);
        }
        for _ in 0..=rotations.len() {
            let Some(next) = rotations.iter().find_map(|rotation| {
                let (old, new) = (rotation.old.array()?, rotation.new.array()?);
                (old == at && rotated(&old, &new, &rotation.word.0)).then_some(new)
            }) else {
                break;
            };
            at = next;
            if at == offered_key {
                let mut hubs = self.hubs.lock().expect("hubs");
                let mut next_hubs = hubs.clone();
                if let Some(pinned) = next_hubs.get_mut(hub.as_str()) {
                    pinned.key = offered_key.into();
                    grow(&mut pinned.lineage, rotations);
                }
                self.save(&next_hubs).map_err(|error| error.to_string())?;
                *hubs = next_hubs;
                return Ok(PublicKey::from_bytes(offered_key));
            }
        }
        Err(format!("the hub {hub} offers another service key than the one this machine pinned, and no rotation it signed leads to it"))
    }

    /// The hub put the history this machine saw aside when it recovered
    /// (design §17): the head it saw is forgotten.
    pub(crate) fn forget_seen(&self, hub: &HubId) -> io::Result<()> {
        let mut hubs = self.hubs.lock().expect("hubs");
        let mut next = hubs.clone();
        if let Some(pinned) = next.get_mut(hub.as_str()) {
            pinned.seen = None;
        }
        self.save(&next)?;
        *hubs = next;
        Ok(())
    }

    /// The latest chain head the hub's word carried here.
    pub(crate) fn seen(&self, hub: &HubId) -> Option<Seen> {
        self.hubs.lock().expect("hubs").get(hub.as_str()).and_then(|pinned| pinned.seen.clone())
    }

    /// The hub's word on `statement`, if it is the pinned key's: the chain
    /// head it carried is kept when it is the latest seen (design §17,
    /// item 4). A word whose head could not be kept on the disk counts for
    /// nothing: what it says may let go of evidence the head stands for.
    pub(crate) fn heard(&self, hub: &HubId, key: &PublicKey, statement: &[u8], word: &[u8]) -> bool {
        let Some((seq, head)) = heard_word(key, statement, word) else { return false };
        let mut hubs = self.hubs.lock().expect("hubs");
        let Some(pinned) = hubs.get(hub.as_str()) else { return false };
        if pinned.seen.as_ref().is_some_and(|seen| seen.seq >= seq) {
            return true;
        }
        let mut next = hubs.clone();
        if let Some(pinned) = next.get_mut(hub.as_str()) {
            pinned.seen = Some(Seen { seq, head: head.into(), statement: statement.to_vec().into(), word: word.to_vec().into() });
        }
        if self.save(&next).is_err() {
            return false;
        }
        *hubs = next;
        true
    }
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

/// This machine as the device that orders its streams.
struct Device(Arc<SecretKey>);

impl Signer for Device {
    fn device(&self) -> [u8; 32] {
        *self.0.public().as_bytes()
    }
    fn key(&self) -> [u8; 32] {
        *self.0.public().as_bytes()
    }
    fn sign(&self, header: &[u8]) -> Vec<u8> {
        self.0.sign(Domain::SyncSourceBatch, header).to_bytes().to_vec()
    }
}

/// Facts under a hub's pinned key, the chain head they carry kept.
pub(crate) struct Pinning<'a> {
    pub(crate) assets: &'a Assets,
    pub(crate) hub: &'a HubId,
    pub(crate) key: PublicKey,
}

impl HubVerifier for Pinning<'_> {
    fn verify(&self, fact: &[u8], signature: &[u8]) -> bool {
        self.assets.heard(self.hub, &self.key, fact, signature)
    }
}
