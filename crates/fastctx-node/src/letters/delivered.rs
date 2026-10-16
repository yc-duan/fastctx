//! One business handoff per immutable letter identity, shared by every hub.

use std::collections::HashSet;
use std::io;
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};

static ACTIVE: LazyLock<Mutex<HashSet<PathBuf>>> = LazyLock::new(Default::default);

pub(crate) struct Handoff(PathBuf);

impl Handoff {
    pub fn begin(node: &crate::Node, identity: &super::Identity) -> io::Result<Self> {
        let path = node.world.path().join("letter-receipts").join(identity.id.as_str());
        if !ACTIVE.lock().expect("never poisoned").insert(path.clone()) {
            return Err(io::Error::new(io::ErrorKind::WouldBlock, "this letter's business handoff is in progress"));
        }
        Ok(Self(path))
    }

    pub fn finished(&self) -> io::Result<bool> {
        match std::fs::read(&self.0) {
            Ok(bytes) if bytes == b"accepted" => {
                self.finish()?;
                Ok(true)
            }
            Ok(_) => {
                Err(io::Error::new(io::ErrorKind::InvalidData, format!("invalid letter receipt: {}", self.0.display())))
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error),
        }
    }

    pub fn finish(&self) -> io::Result<()> {
        fastctx_platform::ensure_private_dir(self.0.parent().expect("receipt directory"))?;
        fastctx_platform::publish(&self.0, b"accepted", fastctx_platform::Permissions::Private)
    }
}

impl Drop for Handoff {
    fn drop(&mut self) {
        let mut active = ACTIVE.lock().expect("never poisoned");
        active.remove(&self.0);
        if active.is_empty() {
            *active = HashSet::new();
        }
    }
}
