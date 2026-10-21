//! Serializes scheduled and on-demand release checks and keeps their discoveries.

use std::path::Path;
use std::sync::Arc;

use fastctx_types::update::Looked;

use crate::Hub;

pub(super) fn read(root: &Path) -> Result<Looked, String> {
    let path = root.join("looked.json");
    match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|error| format!("{}: {error}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Looked::default()),
        Err(error) => Err(format!("{}: {error}", path.display())),
    }
}

pub(super) async fn check(hub: &Arc<Hub>, force: bool) -> Looked {
    let (gate, observed) = {
        let updates = hub.updates.lock().expect("never poisoned");
        (updates.check.clone(), updates.looked.at)
    };
    let _checking = gate.lock().await;
    let (previous, root) = {
        let updates = hub.updates.lock().expect("never poisoned");
        if updates.looked.at != observed || (!force && !fastctx_update::github::wait(&updates.looked).is_zero()) {
            return updates.looked.clone();
        }
        (updates.looked.clone(), updates.cache.root().to_path_buf())
    };
    let looked = fastctx_update::github::look_after(None, &previous, Some(&root.join("github-list.json"))).await;
    if let Some(why) = &looked.why {
        hub.log.line(format_args!("looking for new versions on GitHub: {why}"));
    }
    let saved = serde_json::to_vec(&looked).map_err(|error| error.to_string()).and_then(|bytes| {
        fastctx_platform::publish(&root.join("looked.json"), &bytes, fastctx_platform::Permissions::Private)
            .map_err(|error| error.to_string())
    });
    if let Err(why) = saved {
        hub.log.line(format_args!("keeping discovered releases: {why}"));
    }
    hub.updates.lock().expect("never poisoned").looked = looked.clone();
    super::prefetch(hub);
    super::tell(hub, None);
    looked
}
