//! One business handoff per immutable letter identity, shared by every hub.

use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex, Weak};
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

type Gates = HashMap<PathBuf, Weak<AsyncMutex<()>>>;
static ACTIVE: LazyLock<Mutex<Gates>> = LazyLock::new(Default::default);

pub(crate) struct Handoff {
    path: PathBuf,
    _held: OwnedMutexGuard<()>,
}

impl Handoff {
    pub async fn begin(node: &crate::Node, identity: &super::Identity) -> Self {
        let path = node.world.path().join("letter-receipts").join(identity.id.as_str());
        let gate = {
            let mut active = ACTIVE.lock().expect("never poisoned");
            active.retain(|_, gate| gate.strong_count() != 0);
            if let Some(gate) = active.get(&path).and_then(Weak::upgrade) {
                gate
            } else {
                let gate = Arc::new(AsyncMutex::new(()));
                active.insert(path.clone(), Arc::downgrade(&gate));
                gate
            }
        };
        let held = gate.lock_owned().await;
        Self { path, _held: held }
    }

    pub fn finished(&self) -> io::Result<bool> {
        match std::fs::read(&self.path) {
            Ok(bytes) if bytes == b"accepted" => {
                self.finish()?;
                Ok(true)
            }
            Ok(_) => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("invalid letter receipt: {}", self.path.display()),
            )),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error),
        }
    }

    pub fn finish(&self) -> io::Result<()> {
        fastctx_platform::ensure_private_dir(self.path.parent().expect("receipt directory"))?;
        fastctx_platform::publish(&self.path, b"accepted", fastctx_platform::Permissions::Private)
    }
}

impl Drop for Handoff {
    fn drop(&mut self) {
        let mut active = ACTIVE.lock().expect("never poisoned");
        if active.get(&self.path).is_some_and(|gate| gate.strong_count() == 1) {
            active.remove(&self.path);
        }
        if active.is_empty() {
            *active = HashMap::new();
        }
    }
}
