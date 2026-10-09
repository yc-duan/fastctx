//! An application-owned identity, independent of the local node's directory.
//!
//! ```text
//! <root>/device.lock        held while an application has the identity open
//! <root>/device.key         this device's key, the same on every hub
//! <root>/hubs/<hub id>/     one link's records (`fastctx_net::link::state`)
//! ```

use std::io;
use std::path::{Path, PathBuf};

use fastctx_net::ids::HubId;
use fastctx_net::link::{Membership, NetDir, state};
use fastctx_net::sign::SecretKey;
use fastctx_platform::{FileLock, ensure_private_dir};

pub(super) struct Store {
    root: PathBuf,
    _lock: FileLock,
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

    /// This device's key, made on first use.
    pub fn key(&self) -> io::Result<SecretKey> {
        let path = self.root.join("device.key");
        match SecretKey::load(&path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let key = SecretKey::generate()?;
                key.create(&path)?;
                Ok(key)
            }
            other => other,
        }
    }

    fn hubs(&self) -> PathBuf {
        self.root.join("hubs")
    }

    /// Who came into this device's accounts at `hub` (`fastctx_net::arrivals`).
    pub fn arrivals(&self, hub: &HubId) -> PathBuf {
        self.hubs().join(hub.as_str()).join("arrivals.json")
    }

    pub fn net(&self, hub: &HubId) -> NetDir {
        NetDir::new(self.hubs().join(hub.as_str()))
    }

    /// The memberships kept, one per hub.
    pub fn memberships(&self) -> io::Result<Vec<Membership>> {
        let mut memberships = Vec::new();
        let entries = match std::fs::read_dir(self.hubs()) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(memberships),
            Err(error) => return Err(error),
        };
        for entry in entries {
            let path = entry?.path();
            match state::read::<Membership>(&path.join("membership.json")) {
                Ok(membership) => memberships.push(membership),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        Ok(memberships)
    }

    pub fn keep(&self, membership: &Membership) -> io::Result<()> {
        let net = self.net(&membership.id);
        net.prepare()?;
        state::write(&net.membership(), membership)
    }

    /// Puts a hub's records aside once this device is logged out of it.
    pub fn forget(&self, hub: &HubId) -> io::Result<()> {
        let aside = self.root.join(format!("left-{hub}-{}", jiff::Timestamp::now().strftime("%Y%m%dT%H%M%SZ")));
        std::fs::rename(self.net(hub).path(), aside)
    }
}
