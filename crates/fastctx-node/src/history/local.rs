//! Standalone history has the same revision rules without a network account.
//! Each scope is read through its shelf (`super::shelf`): only what changed
//! since the last call and the text a call needs come from disk, and only
//! the local copy of a document that moved on is written again.

use crate::Node;
use fastctx_types::{
    history::{Author, HistoryCall, HistoryReply, Scope, stored::Stored},
    ipc::Failure,
};
use std::{path::PathBuf, sync::Arc};

use super::shelf::{Of, Place, Shelf};

fn failure(error: impl std::fmt::Display) -> Failure {
    Failure::new(error.to_string())
}

pub(crate) fn directory(node: &Node, scope: &Scope) -> PathBuf {
    match scope {
        Scope::Conversation { resident, machine } => node
            .world
            .owner(&node.names(), &fastctx_types::Address { resident: resident.clone(), machine: machine.clone() })
            .path()
            .join("history-records"),
        Scope::Project { .. } => unreachable!("project history is signed"),
    }
}

fn place(node: &Node, scope: &Scope) -> Place {
    let of = match scope {
        Scope::Project { project } => Of::Project { world: node.world.clone(), town: project.clone() },
        Scope::Conversation { .. } => Of::Conversation { records: directory(node, scope) },
    };
    Place { of, copies: super::local_copy::folder(node, scope) }
}

/// Runs `work` on the shelf and its place on a thread meant to block, and
/// gives both back. A shelf whose work failed is let go: it may hold part
/// of a change, and the next call reads the scope afresh.
async fn on_shelf<R: Send + 'static>(
    shelf: &mut Shelf,
    place: Place,
    work: impl FnOnce(&mut Shelf, &Place) -> Result<R, String> + Send + 'static,
) -> Result<(R, Place), Failure> {
    let mut taken = std::mem::take(shelf);
    let (taken, result, place) = fastctx_platform::task::spawn_blocking(move || {
        let result = work(&mut taken, &place);
        (taken, result, place)
    })
    .await
    .map_err(failure)?;
    match result {
        Ok(value) => {
            *shelf = taken;
            Ok((value, place))
        }
        Err(error) => Err(failure(error)),
    }
}

pub(crate) async fn call(
    node: &Arc<Node>,
    request: HistoryCall,
    author: Option<Author>,
) -> Result<HistoryReply, Failure> {
    let scope = request.scope().clone();
    if request.writes() {
        if super::import::pending(node, &scope) {
            return Err(super::import::writing());
        }
        if matches!(scope, Scope::Conversation { .. })
            && super::import::state::belongs_to_account(&directory(node, &scope)).map_err(failure)?
        {
            return Err(failure("this history belongs to its network account; sign in to continue writing"));
        }
    }
    let identity = serde_json::to_string(&scope).map_err(failure)?;
    let shelf = node.history_shelves.of(&identity);
    let mut shelf = shelf.lock().await;
    let working = node.clone();
    let copies_scope = scope.clone();
    let ((), at) = on_shelf(&mut shelf, place(node, &scope), move |shelf, place| {
        shelf.refresh(place)?;
        if super::import::copies::enabled(&working, &copies_scope)? {
            shelf.keep_copies(place)?;
        }
        Ok(())
    })
    .await?;
    let (name, text, expected, retired) = match request {
        HistoryCall::List { .. } => return Ok(HistoryReply::Catalog { catalog: shelf.catalog() }),
        HistoryCall::Read { name, version, .. } => {
            let (document, _) = on_shelf(&mut shelf, at, move |shelf, place| {
                let mut document = shelf.whole(place, &name)?.ok_or("the history document does not exist")?;
                if let Some(version) = version {
                    let stored = shelf.stored(place, &name, version)?;
                    document.revision = stored.revision;
                    document.text = stored.text;
                }
                Ok(document)
            })
            .await?;
            return Ok(HistoryReply::Document { document: Box::new(document) });
        }
        HistoryCall::Save { name, text, expected, .. } => (name, text, expected, false),
        HistoryCall::Retire { name, expected, .. } => {
            let current = shelf.document(&name).ok_or_else(|| failure("the history document does not exist"))?;
            let version = current.latest.version;
            let latest = name.clone();
            let (stored, _) =
                on_shelf(&mut shelf, place(node, &scope), move |shelf, place| shelf.stored(place, &latest, version))
                    .await?;
            (name, stored.text, Some(expected), true)
        }
        HistoryCall::Restore { name, version, .. } => {
            let current = shelf.document(&name).ok_or_else(|| failure("the history document does not exist"))?;
            let expected = current.latest.version;
            let restored = name.clone();
            let (stored, _) =
                on_shelf(&mut shelf, place(node, &scope), move |shelf, place| shelf.stored(place, &restored, version))
                    .await?;
            (name, stored.text, Some(expected), false)
        }
    };
    let author = author.ok_or_else(|| failure("history writes require attribution"))?;
    let current = shelf.document(&name).map(|document| document.latest.version);
    if expected != current {
        return Err(failure(fastctx_types::history::conflict(current)));
    }
    // What the local copy held, so writing the new text over it leaves no
    // trace of tampering.
    let previous = match current {
        None => None,
        Some(version) => {
            let before = name.clone();
            let (stored, _) =
                on_shelf(&mut shelf, place(node, &scope), move |shelf, place| shelf.stored(place, &before, version))
                    .await?;
            Some(stored.text)
        }
    };
    let (revision, files) =
        fastctx_net::topic_history::revision_files(&name, &text, expected, retired, author, None).map_err(failure)?;
    let digest = fastctx_net::sign::sha256(text.as_bytes());
    match &scope {
        // The new entry is read into the shelf by the next call.
        Scope::Project { project } => crate::ledger::write_memory(node, project, files, None).await?,
        Scope::Conversation { .. } => {
            let folder = directory(node, &scope);
            let stored = Stored { revision: revision.clone(), text: text.clone(), submission: None };
            let _gate = node.syncing.lock().await;
            fastctx_platform::task::spawn_blocking(move || {
                let path = folder.join(&stored.revision.name).join(format!("{}.json", stored.revision.version));
                fastctx_platform::ensure_private_dir(path.parent().expect("revision parent"))
                    .map_err(|error| error.to_string())?;
                // A version is written once: one written meanwhile by another
                // writer is a conflict, never overwritten.
                fastctx_platform::publish_new(
                    &path,
                    &serde_json::to_vec(&stored).map_err(|error| error.to_string())?,
                    fastctx_platform::Permissions::Private,
                )
                .map_err(|error| match error.kind() {
                    std::io::ErrorKind::AlreadyExists => {
                        fastctx_types::history::conflict(Some(stored.revision.version))
                    }
                    _ => error.to_string(),
                })
            })
            .await
            .map_err(failure)?
            .map_err(failure)?;
            shelf.put(revision.clone(), digest, 0);
        }
    }
    let saved = revision.clone();
    let written = (!retired).then_some(text);
    let working = node.clone();
    on_shelf(&mut shelf, place(node, &scope), move |shelf, place| {
        if !super::switch::alone(&working) {
            return Ok(());
        }
        let path = place.copies.join(format!("{}.md", saved.name));
        shelf.write_copy(&path, &saved.name, &saved, written.as_deref(), previous.as_deref())
    })
    .await?;
    crate::context::invalidations::history(node, &scope);
    node.memory_changed();
    // A standalone write may have begun just before its first link was installed.
    super::import::schedule(node);
    Ok(HistoryReply::Saved { revision: Box::new(revision) })
}

/// The text of the newest version of `name` in `scope` whose digest is
/// `sha256`: what a conversation saw of a document before writing it.
pub(crate) async fn seen(node: &Arc<Node>, scope: &Scope, name: &str, sha256: &str) -> Result<Option<String>, Failure> {
    let identity = serde_json::to_string(scope).map_err(failure)?;
    let shelf = node.history_shelves.of(&identity);
    let mut shelf = shelf.lock().await;
    let (name, sha256) = (name.to_string(), sha256.to_string());
    let (text, _) = on_shelf(&mut shelf, place(node, scope), move |shelf, place| {
        shelf.refresh(place)?;
        shelf.seen(place, &name, &sha256)
    })
    .await?;
    Ok(text)
}
