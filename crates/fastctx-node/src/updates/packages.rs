//! Helpers join the same package owner as local updates. Each hub owns its
//! upload, while the device owns the shared download and verified copy.

use std::path::PathBuf;
use std::sync::Arc;

use fastctx_types::update::{Fetch, PackageProgress};
use tokio::sync::watch;

use crate::Node;

pub(crate) async fn relay(
    node: &Arc<Node>,
    fetch: Fetch,
    progress: watch::Sender<PackageProgress>,
) -> Result<PathBuf, String> {
    let cache = super::cache(node);
    if let Some(path) = cache.get(&fetch.version, &fetch.target) {
        return Ok(path);
    }
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
    if !relays && carried.is_none() {
        return Err("this machine neither holds the package nor fetches for others".into());
    }
    let mut heard = |heard: fastctx_update::Progress| {
        progress.send_replace(PackageProgress { received: heard.received, total: heard.total });
    };
    let logging = node.clone();
    let log: fastctx_update::pool::Log = Arc::new(move |message| logging.log.line(format_args!("{message}")));
    let brought = node.updates.packages.relay(&cache, &fetch, carried, &mut heard, log).await?;
    if let Some(speed) = brought.github_speed {
        super::github_speed(node, speed);
    }
    super::report(node);
    Ok(brought.path)
}
