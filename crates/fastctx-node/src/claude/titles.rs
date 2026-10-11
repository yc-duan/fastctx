//! Title changes include idle renames, so observation follows the host file.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use fastctx_hosts::claude::{exchange, titles::Titles};
use fastctx_types::ResidentId;
use fastctx_types::session::{Happened, SessionEvent};
use jiff::Timestamp;

use crate::Node;

pub(super) async fn start(node: &Arc<Node>, resident: &ResidentId, directory: &Path) -> std::io::Result<()> {
    let Some(path) = exchange::transcript(directory)? else { return Ok(()) };
    if node.claude.title_files.lock().expect("never poisoned").contains(&path) {
        return Ok(());
    }
    let (changed, mut changes) = tokio::sync::mpsc::channel(1);
    let watched = path.clone();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        if event.as_ref().map_or(true, |event| event.paths.iter().any(|path| path == &watched)) {
            let _ = changed.try_send(());
        }
    })
    .map_err(std::io::Error::other)?;
    notify::Watcher::watch(
        &mut watcher,
        path.parent().ok_or_else(|| std::io::Error::other("Claude transcript has no parent"))?,
        notify::RecursiveMode::NonRecursive,
    )
    .map_err(std::io::Error::other)?;
    if !node.claude.title_files.lock().expect("never poisoned").insert(path.clone()) {
        return Ok(());
    }
    let node = Arc::downgrade(node);
    let resident = resident.clone();
    tokio::spawn(async move {
        let _watcher = watcher;
        let mut titles = Titles::default();
        loop {
            let Some(node) = node.upgrade() else { return };
            match titles.observe(&path) {
                Ok(Some(title))
                    if node.residents.now(&resident).is_some_and(|now| now.title.as_ref() != Some(&title)) =>
                {
                    let event = SessionEvent { at: Timestamp::now(), happened: Happened::Titled { title } };
                    if let Err(error) = crate::turns::apply(&node, &resident, event).await {
                        node.log.line(format_args!("observing Claude's title: {error}"));
                    }
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => node.log.line(format_args!("reading Claude's title in {}: {error}", path.display())),
            }
            drop(node);
            if changes.recv().await.is_none() {
                return;
            }
        }
    });
    Ok(())
}

pub(super) type Files = std::sync::Mutex<HashSet<PathBuf>>;
