//! Changes published by repair/renewal processes are observed at their
//! atomic file names. In-memory progress is announced by its writers.
use std::sync::Arc;

use crate::Node;

pub(super) fn start(node: &Arc<Node>) -> std::io::Result<()> {
    let updates = super::dir(node);
    fastctx_platform::ensure_private_dir(&updates)?;
    let renewal = fastctx_install::renewal::directory(&node.root);
    fastctx_platform::ensure_private_dir(&renewal)?;
    let observed = updates.clone();
    let handover = renewal.clone();
    let services = node.root.join("hub-services.json");
    let topics = node.topics.clone();
    let mut watcher = fastctx_watch::Watcher::new(move |event: notify::Result<notify::Event>| {
        let relevant = match event {
            Err(_) => true,
            Ok(event) => {
                crate::file_events::changes(&event)
                    && (event.need_rescan()
                        || event.paths.iter().any(|path| {
                            path == &services
                                || path.strip_prefix(&observed).is_ok_and(visible)
                                || path
                                    .strip_prefix(&handover)
                                    .is_ok_and(|path| path == std::path::Path::new("plan.json"))
                        }))
            }
        };
        if relevant {
            topics.changed(fastctx_types::topics::UPDATE);
        }
    })
    .following_links(false);
    watcher
        .watch_within(&updates, |path| path.components().count() <= 3 && visible_directory(path))
        .map_err(std::io::Error::other)?;
    watcher.watch(&renewal, notify::RecursiveMode::NonRecursive).map_err(std::io::Error::other)?;
    watcher.watch(&node.root, notify::RecursiveMode::NonRecursive).map_err(std::io::Error::other)?;
    *node.updates.observer.lock().expect("update observer") = Some(watcher);
    Ok(())
}

fn visible_directory(path: &std::path::Path) -> bool {
    let mut parts = path.iter();
    match parts.next().and_then(|part| part.to_str()) {
        None => true,
        Some("packages") => parts.next().is_none_or(|part| part.to_string_lossy().starts_with('v')),
        Some(_) => path.components().count() == 1,
    }
}

fn visible(path: &std::path::Path) -> bool {
    let name = path.file_name().and_then(|name| name.to_str()).unwrap_or_default();
    if path.components().count() == 1 {
        return matches!(name, "repair.json" | "repair-request.json");
    }
    path.starts_with("packages") && !name.ends_with(".part") && !name.starts_with('.')
}
