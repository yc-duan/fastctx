//! Following a session's own file as Claude Code writes it: its title,
//! idle renames included, and the rows that end its blocks of thinking
//! (`thoughts`).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use fastctx_hosts::claude::tail::{Found, Tail};
use fastctx_hosts::claude::{exchange, thoughts::Thought, titles::Titles};
use fastctx_types::ResidentId;
use fastctx_types::session::{Happened, SessionEvent};
use jiff::Timestamp;

use crate::Node;

pub(super) async fn start(node: &Arc<Node>, resident: &ResidentId, directory: &Path) -> std::io::Result<()> {
    let Some(path) = exchange::transcript(directory)? else { return Ok(()) };
    if node.claude.title_files.lock().expect("never poisoned").contains(&path) {
        return Ok(());
    }
    let folder = path.parent().ok_or_else(|| std::io::Error::other("Claude transcript has no parent"))?;
    // Claude Code makes the transcript's folder with the first prompt's line: until then there is
    // nothing to watch, and the session's next report starts this again.
    if !folder.try_exists()? {
        return Ok(());
    }
    let (changed, mut changes) = tokio::sync::mpsc::channel(1);
    let watched = match path.file_name() {
        Some(name) => crate::file_events::named(folder)?.join(name),
        None => path.clone(),
    };
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        if event.as_ref().map_or(true, |event| {
            event.need_rescan() || crate::file_events::changes(event) && event.paths.iter().any(|path| path == &watched)
        }) {
            let _ = changed.try_send(());
        }
    })
    .map_err(std::io::Error::other)?;
    notify::Watcher::watch(&mut watcher, folder, notify::RecursiveMode::NonRecursive).map_err(std::io::Error::other)?;
    if !node.claude.title_files.lock().expect("never poisoned").insert(path.clone()) {
        return Ok(());
    }
    let node = Arc::downgrade(node);
    let resident = resident.clone();
    tokio::spawn(async move {
        let _watcher = watcher;
        let mut tail = Tail::default();
        let mut titles = Titles::default();
        // What the file held when it was first read, or when it was read
        // again after a rewrite, came before: only later rows are new.
        let mut followed = false;
        loop {
            let Some(node) = node.upgrade() else { return };
            let mut restarted = false;
            let mut thoughts = Vec::new();
            let read = tail.read(&path, |found| {
                match found {
                    Found::Restarted => {
                        titles = Titles::default();
                        restarted = true;
                    }
                    Found::Row(row) => {
                        titles.row(row)?;
                        thoughts.extend(Thought::read(row));
                    }
                }
                Ok(())
            });
            if restarted {
                node.claude.thoughts.restarted(&path);
            }
            let new = followed && !restarted;
            for thought in thoughts {
                if let Some(ready) = node.claude.thoughts.read(&path, thought, new)
                    && let Err(error) = super::thoughts::show(&node, ready).await
                {
                    node.log.line(format_args!("showing Claude's thinking as recorded: {error}"));
                }
            }
            match read {
                Ok(()) => {
                    followed = true;
                    match titles.title() {
                        Some(title)
                            if node.residents.now(&resident).is_some_and(|now| now.title.as_ref() != Some(&title)) =>
                        {
                            let event = SessionEvent { at: Timestamp::now(), happened: Happened::Titled { title } };
                            if let Err(error) = crate::turns::apply(&node, &resident, event).await {
                                node.log.line(format_args!("observing Claude's title: {error}"));
                            }
                        }
                        _ => {}
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => node.log.line(format_args!("following Claude's record in {}: {error}", path.display())),
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
