//! Helpers join the same package owner as local updates. Each hub owns its
//! upload, while the device owns the shared download and verified copy.

use std::path::PathBuf;
use std::sync::Arc;

use fastctx_types::update::{Fetch, PackageProgress};
use tokio::sync::watch;

use crate::Node;

/// Keeps helper deliveries alive until the data connection closes.
pub(crate) struct PackageUse {
    node: Arc<Node>,
    lease: Option<fastctx_update::pool::Lease>,
}

impl Drop for PackageUse {
    fn drop(&mut self) {
        self.lease.take();
        super::prune(&self.node, None);
        super::report(&self.node);
    }
}

pub(crate) fn retain(node: &Arc<Node>, version: &str) -> PackageUse {
    PackageUse { node: node.clone(), lease: Some(node.updates.packages.retain(version)) }
}

pub(crate) async fn relay(
    node: &Arc<Node>,
    fetch: Fetch,
    progress: watch::Sender<PackageProgress>,
) -> Result<PathBuf, String> {
    let cache = super::cache(node);
    let (relays, carried) = {
        let held = node.updates.held.lock().expect("never poisoned");
        let carried = held
            .carried
            .clone()
            .filter(|_| held.app.as_ref().is_some_and(|app| app.version == fetch.version))
            .map(|dir| dir.join(format!("{}.tar.gz", fetch.target)))
            .filter(|path| path.is_file());
        (held.settings.relays, carried)
    };
    if !relays {
        return Err("this machine neither holds the package nor fetches for others".into());
    }
    if let Some(path) = cache.get(&fetch.version, &fetch.target) {
        return Ok(path);
    }
    let mut heard = |heard: fastctx_update::Progress| {
        progress.send_replace(PackageProgress { received: heard.received, total: heard.total });
    };
    let logging = node.clone();
    let log: fastctx_update::pool::Log = Arc::new(move |message| logging.log.line(format_args!("{message}")));
    let brought = node.updates.packages.relay(&cache, &fetch, carried, &mut heard, log).await?;
    if let Some(at) = brought.github_at {
        super::github_reached(node, at);
    }
    if let Some(speed) = brought.github_speed {
        super::github_speed(node, speed);
    }
    super::report(node);
    Ok(brought.path)
}
