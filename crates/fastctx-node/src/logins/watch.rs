//! Keeping the World's tools in the logins as this fastctx writes them while
//! the node runs (`world-agent-logins.md`, 接入跟着版本走): the files the World
//! writes in each login are watched, and a change to any of them, by the
//! user, another program or the host itself, brings the logins up to date
//! again once the writing has settled; an hourly look catches what no watch
//! saw. Nothing is watched while they are being brought up to date, so what
//! that writes does not wake it again.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use fastctx_install::apply::{self, Host};
use tokio::sync::mpsc;

use super::Context;
use crate::Node;

/// How long the files stay unchanged before the logins are looked at.
const SETTLED: Duration = Duration::from_secs(5);
/// How often they are looked at with no change seen.
const EVERY: Duration = Duration::from_secs(60 * 60);

/// Watches the logins the World's tools are in from now on.
pub(crate) fn start(node: &Arc<Node>) {
    let node = Arc::downgrade(node);
    tokio::spawn(async move {
        loop {
            // Held here as well as by the watch, so that where nothing can
            // be watched the hourly look still waits its hour.
            let (changed, mut changes) = mpsc::channel(1);
            let watcher = {
                let Some(node) = node.upgrade() else { return };
                watch(&node, changed.clone())
            };
            tokio::select! {
                _ = changes.recv() => {}
                _ = tokio::time::sleep(EVERY) => {}
            }
            while tokio::time::timeout(SETTLED, changes.recv()).await.is_ok_and(|change| change.is_some()) {}
            drop((watcher, changed));
            let Some(node) = node.upgrade() else { return };
            super::refresh::all(&node).await;
        }
    });
}

/// A watch on the folders of every login the World's tools are in, telling
/// `changed` when one of the files the World writes there changes; none
/// where they cannot be watched, which the hourly look stands in for.
fn watch(node: &Node, changed: mpsc::Sender<()>) -> Option<fastctx_watch::Watcher> {
    let places = Context::of(node).map(|context| context.places).ok()?;
    let integrations = match apply::integrations(&places) {
        Ok(integrations) => integrations,
        Err(error) => {
            node.log.line(format_args!("watching the logins the World's tools are in: {error}"));
            return None;
        }
    };
    let mut watched: BTreeSet<PathBuf> = BTreeSet::new();
    let mut folders = Vec::new();
    for integration in integrations {
        let names: &[&str] = match integration.host {
            Host::Codex => &["config.toml", "AGENTS.md"],
            Host::Claude => &["settings.json", "CLAUDE.md"],
        };
        let mut places = vec![(integration.home.clone(), names)];
        if integration.host == Host::Claude {
            places.push((integration.home.join("plugins"), &["installed_plugins.json"][..]));
        }
        for (folder, names) in places {
            let Ok(named) = crate::file_events::named(&folder) else { continue };
            watched.extend(names.iter().map(|name| named.join(name)));
            folders.push(folder);
        }
    }
    let mut watcher = fastctx_watch::Watcher::new(move |event: notify::Result<notify::Event>| {
        if event.as_ref().map_or(true, |event| {
            event.need_rescan()
                || crate::file_events::changes(event) && event.paths.iter().any(|path| watched.contains(path))
        }) {
            let _ = changed.try_send(());
        }
    });
    for folder in folders {
        // A folder not made yet, as Claude Code's plugins before any, is
        // looked at hourly instead.
        let _ = watcher.watch(&folder, notify::RecursiveMode::NonRecursive);
    }
    Some(watcher)
}
