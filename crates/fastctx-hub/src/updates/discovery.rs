//! Serializes scheduled and on-demand release checks and keeps their discoveries.

use std::path::Path;
use std::sync::Arc;

use fastctx_types::update::Looked;

use crate::Hub;

pub(super) fn read(root: &Path) -> Result<Looked, String> {
    let path = root.join("looked.json");
    match fastctx_platform::disk::read(&path) {
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
    let saved = match serde_json::to_vec(&looked) {
        Ok(bytes) => fastctx_platform::task::spawn_blocking(move || {
            fastctx_platform::publish(&root.join("looked.json"), &bytes, fastctx_platform::Permissions::Private)
                .map_err(|error| error.to_string())
        })
        .await
        .unwrap_or_else(|error| Err(error.to_string())),
        Err(error) => Err(error.to_string()),
    };
    if let Err(why) = saved {
        hub.log.line(format_args!("keeping discovered releases: {why}"));
    }
    hub.updates.lock().expect("never poisoned").looked = looked.clone();
    hub.blocking(|hub| super::prefetch(&hub)).await;
    super::tell(hub, None);
    looked
}
