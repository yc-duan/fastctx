//! An application-owned identity, independent of the local node's directory.

use std::io;
use std::path::{Path, PathBuf};

use fastctx_net::link::{Keys, Membership, NetDir, state};
use fastctx_net::seal::SealKey;
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

    pub fn create(&self, membership: Membership) -> io::Result<(Identity, Keys)> {
        let directory = fastctx_net::sign::random::<8>()?.iter().map(|byte| format!("{byte:02x}")).collect();
        let identity = Identity { directory, membership, pending: true };
        ensure_private_dir(&self.directory(&identity))?;
        let key = SecretKey::generate()?;
        key.create(&self.directory(&identity).join("device.key"))?;
        let seal = SealKey::generate()?;
        seal.create(&self.directory(&identity).join("seal.key"))?;
        // Admission can consume a one-use invitation before its reply reaches
        // us. Persist the keys and hub coordinates before attempting it.
        self.save(Some(&identity))?;
        Ok((identity, Keys { sign: key, seal }))
    }

    /// The identity's keys. A device that joined before sealing makes its
    /// sealing key now, once, and the link gives the hub its public half.
    pub fn keys(&self, identity: &Identity) -> io::Result<Keys> {
        let sign = SecretKey::load(&self.directory(identity).join("device.key"))?;
        let path = self.directory(identity).join("seal.key");
        let seal = match SealKey::load(&path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let seal = SealKey::generate()?;
                seal.create(&path)?;
                seal
            }
            other => other?,
        };
        Ok(Keys { sign, seal })
    }

    pub fn net(&self, identity: &Identity) -> NetDir {
        NetDir::new(self.directory(identity).join("net"))
    }

    fn directory(&self, identity: &Identity) -> PathBuf {
        self.root.join(&identity.directory)
    }
}
