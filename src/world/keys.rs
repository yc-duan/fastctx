//! The World key ring: one 32-byte key per epoch, held by every member and never by the hub.
//!
//! Revoking a member rotates to a new epoch. New messages always use the newest epoch; older
//! epochs stay on the ring so history remains readable. A rotated key reaches each remaining
//! member sealed to that member's X25519 wrap key.

use super::crypto::{self, Key32, SubKeys, b64_array, b64_decode, b64_encode};
use super::identity::{Identity, agree_ephemeral};
use super::{WorldPaths, read_optional, write_atomic};
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, ZeroizeOnDrop};

const RING_FILE_VERSION: u32 = 1;

/// One World key epoch.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub(crate) struct WorldKey {
    epoch: u32,
    key: Key32,
}

impl WorldKey {
    pub(crate) fn generate(epoch: u32) -> Result<Self, String> {
        Ok(Self {
            epoch,
            key: crypto::random_bytes::<32>()?,
        })
    }

    pub(crate) fn from_parts(epoch: u32, key: Key32) -> Self {
        Self { epoch, key }
    }

    pub(crate) const fn epoch(&self) -> u32 {
        self.epoch
    }

    pub(crate) fn subkeys(&self) -> SubKeys {
        SubKeys::derive(&self.key)
    }

    #[cfg(test)]
    pub(crate) fn bytes(&self) -> &Key32 {
        &self.key
    }
}

impl std::fmt::Debug for WorldKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "WorldKey(epoch {})", self.epoch)
    }
}

/// Every epoch this member holds, ordered by epoch.
#[derive(Clone, Debug, Default)]
pub(crate) struct KeyRing {
    keys: Vec<WorldKey>,
}

#[derive(Deserialize, Serialize)]
struct RingFile {
    v: u32,
    keys: Vec<RingEntry>,
}

#[derive(Deserialize, Serialize)]
struct RingEntry {
    epoch: u32,
    key: String,
}

impl KeyRing {
    /// A brand-new World: epoch 1 only.
    pub(crate) fn new_initial() -> Result<Self, String> {
        Ok(Self {
            keys: vec![WorldKey::generate(1)?],
        })
    }

    pub(crate) fn from_keys(mut keys: Vec<WorldKey>) -> Result<Self, String> {
        keys.sort_by_key(WorldKey::epoch);
        keys.dedup_by_key(|key| key.epoch);
        if keys.is_empty() {
            return Err("The World key ring is empty.".to_string());
        }
        Ok(Self { keys })
    }

    /// The newest epoch, used for everything sent from now on.
    pub(crate) fn current(&self) -> &WorldKey {
        self.keys.last().expect("a key ring is never empty")
    }

    pub(crate) fn get(&self, epoch: u32) -> Option<&WorldKey> {
        self.keys.iter().find(|key| key.epoch == epoch)
    }

    pub(crate) fn subkeys(&self, epoch: u32) -> Option<SubKeys> {
        self.get(epoch).map(WorldKey::subkeys)
    }

    pub(crate) fn epochs(&self) -> Vec<u32> {
        self.keys.iter().map(WorldKey::epoch).collect()
    }

    /// Adds an epoch learned from another member; an epoch already present must match.
    pub(crate) fn add(&mut self, key: WorldKey) -> Result<(), String> {
        if let Some(existing) = self.get(key.epoch) {
            if existing.key == key.key {
                return Ok(());
            }
            return Err(format!(
                "A different World key already exists for epoch {}.",
                key.epoch
            ));
        }
        self.keys.push(key);
        self.keys.sort_by_key(WorldKey::epoch);
        Ok(())
    }

    /// Creates the next epoch locally; the caller distributes it.
    pub(crate) fn rotate(&mut self) -> Result<&WorldKey, String> {
        let next = self.current().epoch + 1;
        self.keys.push(WorldKey::generate(next)?);
        Ok(self.current())
    }

    pub(crate) fn load(paths: &WorldPaths) -> Result<Option<Self>, String> {
        let Some(bytes) = read_optional(&paths.world_keys)? else {
            return Ok(None);
        };
        let file: RingFile = serde_json::from_slice(&bytes).map_err(|error| {
            format!(
                "Cannot parse {}: {error}",
                crate::paths::display_path(&paths.world_keys)
            )
        })?;
        if file.v > RING_FILE_VERSION {
            return Err(format!(
                "{} was written by a newer fastctx (format {}); this build reads format {} at most.",
                crate::paths::display_path(&paths.world_keys),
                file.v,
                RING_FILE_VERSION
            ));
        }
        let keys = file
            .keys
            .iter()
            .map(|entry| {
                Ok(WorldKey::from_parts(
                    entry.epoch,
                    crypto::hex_array::<32>(&entry.key, "World key")?,
                ))
            })
            .collect::<Result<Vec<_>, String>>()?;
        Self::from_keys(keys).map(Some)
    }

    pub(crate) fn save(&self, paths: &WorldPaths) -> Result<(), String> {
        let file = RingFile {
            v: RING_FILE_VERSION,
            keys: self
                .keys
                .iter()
                .map(|key| RingEntry {
                    epoch: key.epoch,
                    key: hex::encode(key.key),
                })
                .collect(),
        };
        let json = serde_json::to_vec_pretty(&file)
            .map_err(|error| format!("Cannot encode the World key ring: {error}"))?;
        write_atomic(&paths.world_keys, &json)
    }

    /// Serializes every epoch for wrapping inside an invite or a key export.
    pub(crate) fn to_plain_json(&self) -> Vec<u8> {
        let entries = self
            .keys
            .iter()
            .map(|key| RingEntry {
                epoch: key.epoch,
                key: hex::encode(key.key),
            })
            .collect::<Vec<_>>();
        serde_json::to_vec(&entries).expect("ring entries serialize")
    }

    pub(crate) fn from_plain_json(bytes: &[u8]) -> Result<Self, String> {
        let entries: Vec<RingEntry> = serde_json::from_slice(bytes)
            .map_err(|error| format!("Cannot parse the wrapped World keys: {error}"))?;
        let keys = entries
            .iter()
            .map(|entry| {
                Ok(WorldKey::from_parts(
                    entry.epoch,
                    crypto::hex_array::<32>(&entry.key, "World key")?,
                ))
            })
            .collect::<Result<Vec<_>, String>>()?;
        Self::from_keys(keys)
    }
}

/// A World key epoch sealed to one member's wrap key (X25519 + HKDF + XChaCha20-Poly1305).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct SealedKey {
    pub(crate) epoch: u32,
    /// The sender's one-shot X25519 public key, base64.
    pub(crate) ephemeral_public: String,
    /// Base64 `nonce ‖ ciphertext` of the 32-byte key, AAD = the epoch and both public keys.
    pub(crate) sealed: String,
}

impl SealedKey {
    /// Seals `key` so only the holder of `recipient_wrap_public`'s secret can open it.
    pub(crate) fn seal(key: &WorldKey, recipient_wrap_public: &Key32) -> Result<Self, String> {
        let (ephemeral_public, shared) = agree_ephemeral(recipient_wrap_public)?;
        let wrap_key = wrap_key(&shared, &ephemeral_public, recipient_wrap_public);
        let aad = aad(key.epoch, &ephemeral_public, recipient_wrap_public);
        let sealed = crypto::seal(&wrap_key, &aad, &key.key)?;
        Ok(Self {
            epoch: key.epoch,
            ephemeral_public: b64_encode(&ephemeral_public),
            sealed: b64_encode(&sealed.to_bytes()),
        })
    }

    /// Opens a key sealed to this member.
    pub(crate) fn open(&self, identity: &Identity) -> Result<WorldKey, String> {
        let ephemeral_public = b64_array::<32>(&self.ephemeral_public, "ephemeral key")?;
        let recipient_public = identity.wrap_public();
        let shared = identity.agree(&ephemeral_public)?;
        let wrap_key = wrap_key(&shared, &ephemeral_public, &recipient_public);
        let aad = aad(self.epoch, &ephemeral_public, &recipient_public);
        let sealed = crypto::Sealed::from_bytes(&b64_decode(&self.sealed)?)?;
        let plain =
            crypto::open(&wrap_key, &sealed.nonce, &aad, &sealed.ciphertext).map_err(|_| {
                format!(
                    "The sealed World key for epoch {} does not open with this member's wrap key.",
                    self.epoch
                )
            })?;
        let key = Key32::try_from(plain)
            .map_err(|_| "The sealed World key has the wrong length.".to_string())?;
        Ok(WorldKey::from_parts(self.epoch, key))
    }
}

fn wrap_key(shared: &Key32, ephemeral_public: &Key32, recipient_public: &Key32) -> Key32 {
    let mut label = Vec::with_capacity(8 + 64);
    label.extend_from_slice(b"key-wrap");
    label.extend_from_slice(ephemeral_public);
    label.extend_from_slice(recipient_public);
    crypto::hkdf_derive(shared, &label)
}

fn aad(epoch: u32, ephemeral_public: &Key32, recipient_public: &Key32) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(4 + 64);
    bytes.extend_from_slice(&epoch.to_be_bytes());
    bytes.extend_from_slice(ephemeral_public);
    bytes.extend_from_slice(recipient_public);
    bytes
}

/// Seals arbitrary bytes under a symmetric key with a random nonce, base64 `nonce ‖ ciphertext`.
pub(crate) fn seal_blob(key: &Key32, aad: &[u8], plaintext: &[u8]) -> Result<String, String> {
    Ok(b64_encode(&crypto::seal(key, aad, plaintext)?.to_bytes()))
}

/// Opens a blob produced by `seal_blob`.
pub(crate) fn open_blob(key: &Key32, aad: &[u8], blob: &str) -> Result<Vec<u8>, String> {
    let sealed = crypto::Sealed::from_bytes(&b64_decode(blob)?)?;
    crypto::open(key, &sealed.nonce, aad, &sealed.ciphertext)
}

#[cfg(test)]
mod tests {
    use super::{KeyRing, SealedKey, WorldKey};
    use crate::world::identity::Identity;

    #[test]
    fn a_rotated_key_reaches_only_the_member_it_was_sealed_for() {
        let mut ring = KeyRing::new_initial().unwrap();
        let rotated = ring.rotate().unwrap().clone();
        assert_eq!(ring.current().epoch(), 2);
        assert_eq!(ring.epochs(), vec![1, 2]);

        let alice = Identity::generate().unwrap();
        let bob = Identity::generate().unwrap();
        let sealed = SealedKey::seal(&rotated, &alice.wrap_public()).unwrap();
        let opened = sealed.open(&alice).unwrap();
        assert_eq!(opened.epoch(), 2);
        assert_eq!(opened.bytes(), rotated.bytes());
        assert!(sealed.open(&bob).is_err());

        let mut tampered = sealed.clone();
        tampered.epoch = 3;
        assert!(tampered.open(&alice).is_err());
    }

    #[test]
    fn the_ring_file_and_plain_json_round_trip_every_epoch() {
        let temp = tempfile::tempdir().unwrap();
        let paths = crate::world::WorldPaths::from_control(
            &crate::control::paths::ControlPaths::for_home(temp.path()),
        );
        paths.ensure().unwrap();
        let mut ring = KeyRing::new_initial().unwrap();
        ring.rotate().unwrap();
        ring.save(&paths).unwrap();
        let loaded = KeyRing::load(&paths).unwrap().unwrap();
        assert_eq!(loaded.epochs(), ring.epochs());
        assert_eq!(loaded.current().bytes(), ring.current().bytes());
        let plain = KeyRing::from_plain_json(&ring.to_plain_json()).unwrap();
        assert_eq!(plain.epochs(), vec![1, 2]);
        assert!(
            loaded
                .clone()
                .add(WorldKey::from_parts(1, [9_u8; 32]))
                .is_err()
        );
    }
}
