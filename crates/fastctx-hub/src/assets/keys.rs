//! The hub's service key and its keyring (design §6.1): made with an
//! Ed25519 seed the first time the hub opens, kept in `keys/` readable by
//! the hub's account alone, beside the keys it retired; a key is retired,
//! never deleted. Everything the hub says of assets is its word under the
//! key in use, bound to the chain head the witness holds
//! (`fastctx_net::sign::hub_word`); a word under a retired key still counts
//! as the hub's.
//!
//! A rotation (`fastctx hub rotate-key`) makes the next key and the old
//! key's signed word on it, and exports the keyring with the next key in it
//! before the next key is put in use: a backup without it could not sign
//! for the devices that pinned it (§17).

use super::witness::Witness;
use fastctx_content::v2::HubSigner;
use fastctx_net::sign::{Domain, PublicKey, SecretKey, heard_word, hub_word};
use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The file of the key in use within the keyring.
const CURRENT: &str = "current.key";
/// The next key, made by a rotation not yet put in use.
const NEXT: &str = "next.key";
/// The keys retired, each by its public key.
const RETIRED: &str = "retired";
/// Each rotation the hub made, the old key's word on the new.
const ROTATIONS: &str = "rotations.json";

/// The old key's word that the new key follows it (design §6.1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Rotation {
    pub old: [u8; 32],
    pub new: [u8; 32],
    pub signature: Vec<u8>,
}

/// What the old key signs of a rotation.
pub(crate) fn rotation_statement(old: &[u8; 32], new: &[u8; 32]) -> Vec<u8> {
    let mut statement = b"fastctx-hub-key-rotation/1".to_vec();
    statement.extend_from_slice(old);
    statement.extend_from_slice(new);
    statement
}

/// The keyring: the key in use, those retired, and the rotations.
pub(crate) struct Keys {
    current: SecretKey,
    retired: Vec<PublicKey>,
    /// The next key a rotation made and did not put in use yet: a device
    /// may have pinned it from an export (§17).
    next: Option<PublicKey>,
    rotations: Vec<Rotation>,
}

fn hex(bytes: &[u8]) -> String {
    super::witness::hex(bytes)
}

impl Keys {
    /// The keyring in `dir`, its first key made when there is none.
    pub(crate) fn open(dir: &Path) -> io::Result<Self> {
        fastctx_platform::ensure_private_dir(dir)?;
        let path = dir.join(CURRENT);
        let current = if fastctx_platform::disk::exists(&path)? {
            SecretKey::load(&path)?
        } else {
            let key = SecretKey::generate()?;
            key.create(&path)?;
            key
        };
        let mut retired = Vec::new();
        match fastctx_platform::disk::read_dir(dir.join(RETIRED)) {
            Ok(entries) => {
                for entry in entries {
                    retired.push(SecretKey::load(&entry?.path())?.public());
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let rotations = match fastctx_platform::disk::read(dir.join(ROTATIONS)) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(error) => return Err(error),
        };
        let next_path = dir.join(NEXT);
        let next = if fastctx_platform::disk::exists(&next_path)? { Some(SecretKey::load(&next_path)?.public()) } else { None };
        Ok(Self { current, retired, next, rotations })
    }

    pub(crate) fn public(&self) -> PublicKey {
        self.current.public()
    }

    /// Whether `key` is in the keyring, in use or retired.
    pub(crate) fn holds(&self, key: &[u8; 32]) -> bool {
        self.current.public().as_bytes() == key
            || self.retired.iter().any(|retired| retired.as_bytes() == key)
            || self.next.is_some_and(|next| next.as_bytes() == key)
    }

    pub(crate) fn rotations(&self) -> &[Rotation] {
        &self.rotations
    }

    /// The chain head a word of the hub's on `statement` carries, under any
    /// key of its keyring.
    pub(crate) fn heard(&self, statement: &[u8], word: &[u8]) -> Option<(u64, [u8; 32])> {
        std::iter::once(self.current.public())
            .chain(self.retired.iter().copied())
            .find_map(|key| heard_word(&key, statement, word))
    }
}

/// A rotation made and not yet in use: the next key's public half.
pub(crate) fn prepare(dir: &Path, export: &Path) -> io::Result<PublicKey> {
    let keys = Keys::open(dir)?;
    let next_path = dir.join(NEXT);
    let next = if fastctx_platform::disk::exists(&next_path)? {
        SecretKey::load(&next_path)?
    } else {
        let key = SecretKey::generate()?;
        key.create(&next_path)?;
        key
    };
    let (old, new) = (*keys.current.public().as_bytes(), *next.public().as_bytes());
    let signature = keys.current.sign(Domain::SyncHubKeyRotation, &rotation_statement(&old, &new)).to_bytes().to_vec();
    let mut rotations = keys.rotations.clone();
    rotations.retain(|rotation| !(rotation.old == old && rotation.new == new));
    rotations.push(Rotation { old, new, signature });
    let bytes = serde_json::to_vec_pretty(&rotations).map_err(io::Error::other)?;
    fastctx_platform::publish(&dir.join(ROTATIONS), &bytes, fastctx_platform::Permissions::Private)?;
    copy_tree(dir, export)?;
    Ok(next.public())
}

/// Puts the next key a rotation made in use, the key in use retired.
pub(crate) fn finish(dir: &Path) -> io::Result<(PublicKey, PublicKey)> {
    let next_path = dir.join(NEXT);
    if !fastctx_platform::disk::exists(&next_path)? {
        return Err(io::Error::other("no rotation is prepared: run it with --export first"));
    }
    let current = SecretKey::load(&dir.join(CURRENT))?;
    let next = SecretKey::load(&next_path)?;
    let retired = dir.join(RETIRED);
    fastctx_platform::ensure_private_dir(&retired)?;
    // Whole or not at all: a retired copy cut short would keep the keyring
    // from opening.
    let bytes = fastctx_platform::disk::read(dir.join(CURRENT))?;
    fastctx_platform::publish(&retired.join(format!("{}.key", hex(current.public().as_bytes()))), &bytes, fastctx_platform::Permissions::Private)?;
    fastctx_platform::disk::rename(&next_path, dir.join(CURRENT))?;
    Ok((current.public(), next.public()))
}

/// Copies the directory `from` to `to`, which must not hold anything yet
/// nor lie within `from`: every directory made private, every file readable
/// by the hub's account alone and on the disk before this returns. A
/// SQLite database is copied as it is committed (`VACUUM INTO`), whatever
/// its log holds, and its log is not copied.
pub(crate) fn copy_tree(from: &Path, to: &Path) -> io::Result<()> {
    if fastctx_platform::disk::exists(to)? && fastctx_platform::disk::read_dir(to)?.next().is_some() {
        return Err(io::Error::other(format!("{} holds something already", to.display())));
    }
    if super::witness::inside(to, from)? {
        return Err(io::Error::other(format!("{} lies within {}", to.display(), from.display())));
    }
    fastctx_platform::ensure_private_dir(to)?;
    let mut made = vec![to.to_path_buf()];
    let mut pending: Vec<(PathBuf, PathBuf)> = vec![(from.to_path_buf(), to.to_path_buf())];
    while let Some((source, target)) = pending.pop() {
        for entry in fastctx_platform::disk::read_dir(&source)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            let name = entry.file_name();
            let place = target.join(&name);
            let text = name.to_string_lossy();
            if kind.is_dir() {
                fastctx_platform::ensure_private_dir(&place)?;
                made.push(place.clone());
                pending.push((entry.path(), place));
            } else if kind.is_file() {
                if text.ends_with("-wal") || text.ends_with("-shm") || text.ends_with("-journal") {
                    continue;
                }
                if text.ends_with(".sqlite3") {
                    let source = fastctx_storage::rusqlite::Connection::open_with_flags(
                        entry.path(),
                        fastctx_storage::rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
                    )
                    .map_err(io::Error::other)?;
                    source.execute("VACUUM INTO ?1", [place.to_string_lossy().as_ref()]).map_err(io::Error::other)?;
                } else {
                    fastctx_platform::disk::copy(entry.path(), &place)?;
                }
                private(&place)?;
                // Flushed through a handle that may write, as the system asks.
                fastctx_platform::disk::open_with(std::fs::OpenOptions::new().write(true), &place)?.sync_all()?;
            }
        }
    }
    for directory in made {
        sync_directory(&directory);
    }
    Ok(())
}

/// Makes a copied file readable by the hub's account alone.
fn private(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// Puts a directory's entries on the disk, where the system can.
fn sync_directory(path: &Path) {
    #[cfg(unix)]
    if let Ok(directory) = fastctx_platform::disk::open(path) {
        let _ = directory.sync_all();
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// Signs as the hub, bound to the chain head the witness holds.
pub(crate) struct Signer {
    pub(crate) keys: Arc<Keys>,
    pub(crate) witness: Arc<Witness>,
}

impl HubSigner for Signer {
    fn sign(&self, statement: &[u8]) -> Vec<u8> {
        let (seq, head) = self.witness.current();
        hub_word(&self.keys.current, seq, &head, statement)
    }
}
