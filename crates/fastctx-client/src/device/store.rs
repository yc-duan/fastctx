//! An application-owned identity, independent of the local node's directory.

use std::io;
use std::path::{Path, PathBuf};

use fastctx_net::link::{Membership, NetDir, state};
use fastctx_net::sign::SecretKey;
use fastctx_platform::{FileLock, ensure_private_dir};
use serde::{Deserialize, Serialize};

pub(super) struct Store {
    root: PathBuf,
    _lock: FileLock,
}

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Identity {
    directory: String,
    pub membership: Membership,
    pub pending: bool,
}

impl Store {
    pub fn open(root: PathBuf) -> io::Result<Self> {
        ensure_private_dir(&root)?;
        let lock = FileLock::try_acquire(&root.join("device.lock"))?
            .ok_or_else(|| io::Error::other("this device identity is already open in another application"))?;
        Ok(Self { root, _lock: lock })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn load(&self) -> io::Result<Option<Identity>> {
        let identity: Option<Identity> = state::read_or(&self.root.join("identity.json"), None)?;
        if let Some(identity) = &identity
            && (identity.directory.len() != 16 || !identity.directory.bytes().all(|byte| byte.is_ascii_hexdigit()))
        {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid device identity directory"));
        }
        Ok(identity)
    }

    pub fn save(&self, identity: Option<&Identity>) -> io::Result<()> {
        state::write(&self.root.join("identity.json"), &identity)
    }

    pub fn create(&self, membership: Membership) -> io::Result<(Identity, SecretKey)> {
        let directory = fastctx_net::sign::random::<8>()?.iter().map(|byte| format!("{byte:02x}")).collect();
        let identity = Identity { directory, membership, pending: true };
        ensure_private_dir(&self.directory(&identity))?;
        let key = SecretKey::generate()?;
        key.create(&self.directory(&identity).join("device.key"))?;
        // Admission can consume a one-use invitation before its reply reaches
        // us. Persist the key and hub coordinates before attempting it.
        self.save(Some(&identity))?;
        Ok((identity, key))
    }

    pub fn key(&self, identity: &Identity) -> io::Result<SecretKey> {
        SecretKey::load(&self.directory(identity).join("device.key"))
    }

    pub fn net(&self, identity: &Identity) -> NetDir {
        NetDir::new(self.directory(identity).join("net"))
    }

    fn directory(&self, identity: &Identity) -> PathBuf {
        self.root.join(&identity.directory)
    }
}
