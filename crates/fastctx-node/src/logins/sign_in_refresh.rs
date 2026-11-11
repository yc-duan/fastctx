//! Re-identifies changed sign-ins independently of reads of the login list.
use super::{Context, kept, sign_in};
use crate::{Node, passes};
use std::collections::BTreeSet;

use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

const PASS: passes::Pass = passes::Pass {
    name: "logins/sign-in",
    within: Duration::from_secs(10 * 60),
    rests: Some(Duration::from_secs(60 * 60)),
};
const SETTLED: Duration = Duration::from_millis(200);

pub(super) fn ask(node: &Node) {
    passes::ask(node, PASS.name);
}

/// Starts after recovery, asking for an initial pass as well as watching files.
pub(crate) fn register(node: &Arc<Node>) {
    super::snapshot::register(node);
    passes::register(node, PASS, |node| async move {
        // Quota-only projections must not postpone the hourly full reading.
        super::snapshot::ask(&node);
        sign_in::refresh(&node).await
    });
    let weak = Arc::downgrade(node);
    let rewatch = node.logins.sign_in_watch.clone();
    fastctx_platform::task::spawn(async move {
        let (changed, mut changes) = mpsc::channel(1);
        loop {
            let Some(node) = weak.upgrade() else { return };
            let watching = node.clone();
            let sender = changed.clone();
            let watcher = fastctx_platform::task::spawn_blocking(move || watch(&watching, sender)).await;
            drop(node);
            tokio::select! {
                _ = changes.recv() => {}
                () = rewatch.notified() => continue,
            }
            tokio::time::sleep(SETTLED).await;
            while changes.try_recv().is_ok() {}
            let Some(node) = weak.upgrade() else { return };
            ask(&node);
            super::snapshot::ask(&node);
            // A missing login directory may just have been created. Rebuild
            // its watch before waiting for the next change.
            drop(watcher);
        }
    });
}

fn watch(node: &Node, changed: mpsc::Sender<()>) -> Option<fastctx_watch::Watcher> {
    let context = match Context::of(node) {
        Ok(context) => context,
        Err(error) => {
            node.log.line(format_args!("watching sign-in files: {}", error.message));
            return None;
        }
    };
    let record = match kept::read(node) {
        Ok(record) => record,
        Err(error) => {
            node.log.line(format_args!("watching sign-in files: {error}"));
            return None;
        }
    };
    let mut files = BTreeSet::new();
    let mut folders = BTreeSet::new();
    for agent in super::AGENTS {
        for login in &record.of(agent).logins {
            for file in sign_in::files(&context, agent, &login.dir) {
                let Some(parent) = file.parent() else { continue };
                // Watching the nearest existing ancestor also notices a login
                // directory made after startup, without recursively watching it.
                let Some(folder) = parent.ancestors().find(|dir| fastctx_platform::disk::is_dir(dir)) else {
                    continue;
                };
                let Ok(named) = crate::file_events::named(folder) else { continue };
                let Ok(relative) = file.strip_prefix(folder) else { continue };
                files.insert(named.join(relative));
                folders.insert(folder.to_path_buf());
            }
        }
    }
    let mut watcher = fastctx_watch::Watcher::new(move |event: notify::Result<notify::Event>| {
        if event.as_ref().map_or(true, |event| {
            event.need_rescan()
                || crate::file_events::changes(event)
                    && event.paths.iter().any(|path| files.iter().any(|file| file.starts_with(path)))
        }) {
            let _ = changed.try_send(());
        }
    });
    for folder in folders {
        if let Err(error) = watcher.watch(&folder, notify::RecursiveMode::NonRecursive) {
            node.log.line(format_args!("watching sign-in files in {}: {error}", folder.display()));
        }
    }
    Some(watcher)
}
