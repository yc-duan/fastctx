//! The only path from a mutable replica to a new authoritative revision.

use super::{
    bodies::Text,
    state::{self, Copy, Observed, Pending, Slot},
    sync,
};
use crate::{Node, town::caller::Caller};
use fastctx_types::{
    Address, JobId, TownName, history,
    ipc::Failure,
    replicas::{Document, Head, Scope},
    write::{ChangeKind, FileChange, WriteError, WriteOp, WriteOutcome, WriteResult},
};
use std::{path::Path, sync::Arc};

/// How many edits of one document wait for its hub at most. Each becomes a
/// revision of its own, so none is folded into another; past this many an
/// edit is refused until the document reaches its hub.
const WAITING_EDITS: usize = 64;

fn failure(error: impl std::fmt::Display) -> Failure {
    Failure::new(error.to_string())
}

async fn locate(node: &Arc<Node>, path: &Path) -> Result<Option<Slot>, Failure> {
    locate_inner(node, path, true).await
}

async fn locate_inner(node: &Arc<Node>, path: &Path, refresh: bool) -> Result<Option<Slot>, Failure> {
    node.replicas.state.load(&node.world).map_err(failure)?;
    let native = fastctx_platform::local_path(path).map_err(failure)?;
    if !native.is_absolute() {
        return Ok(None);
    }
    let actual = fastctx_hands::files::canonical_path(&native).map_err(failure)?;
    let lexical = node.replicas.state.at(&native);
    let resolved = node.replicas.state.at(&actual);
    if lexical.is_some() && resolved.is_none() {
        return Err(Failure::refused("the document replica was redirected; read its restored copy before editing"));
    }
    if let Some(slot) = resolved.or(lexical) {
        return Ok(Some(slot));
    }
    let parent = native.parent().ok_or_else(|| failure("a history path has no parent"))?;
    let folder = node
        .replicas
        .state
        .folders
        .lock()
        .expect("never poisoned")
        .values()
        .find(|folder| state::path_key(&folder.path) == state::path_key(parent))
        .cloned();
    let Some(folder) = folder else {
        let root = state::path_key(node.world.path());
        let path_key = state::path_key(&native);
        let protected = path_key.strip_prefix(&format!("{root}/")).is_some_and(|relative| {
            let parts: Vec<_> = relative.split('/').collect();
            matches!(
                parts.as_slice(),
                ["conversations", _, "plan.md"] | ["conversations", _, "history", _] | ["towns", _, "history", _]
            )
        });
        let modern: Vec<_> = node.links().into_iter().filter(|link| link.view().shared_records).collect();
        if protected && !modern.is_empty() {
            if refresh {
                for link in modern {
                    super::sync::all(node, &link).await.map_err(failure)?;
                }
                return Box::pin(locate_inner(node, &native, false)).await;
            }
            return Err(Failure::not_run("this record has no writable replica here; use the path the World supplied"));
        }
        return Ok(None);
    };
    if actual.parent().map(state::path_key)
        != Some(state::path_key(&fastctx_hands::files::canonical_path(&folder.path).map_err(failure)?))
    {
        return Err(Failure::refused("the history file was redirected outside its folder"));
    }
    let name = native
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_suffix(".md"))
        .ok_or_else(|| failure("history files use the .md extension"))?;
    history::markdown::check_name(name).map_err(failure)?;
    let link = node.link_of(&folder.account.hub).ok_or_else(|| failure("the history's hub is unavailable"))?;
    let view = link.view();
    let owner = match &folder.scope {
        Scope::Project { town, .. } => {
            view.project(town).ok_or_else(|| failure("the project was removed"))?.owner.clone()
        }
        Scope::Conversation { conversation } => conversation.account.clone(),
    };
    let opened = view.account(&owner).ok_or_else(|| failure("the history's owning account is unavailable"))?.opened;
    let head = Head {
        owner,
        opened,
        document: Document::History { scope: folder.scope, name: name.into() },
        version: None,
        retired: false,
        history: None,
    };
    Ok(Some(node.replicas.state.insert(Copy {
        epoch: 0,
        account: folder.account,
        head,
        path: native,
        body: Text::default(),
        projected: None,
        observed: Default::default(),
        pending: Vec::new(),
        left: Vec::new(),
    })))
}

fn authorize(node: &Node, caller: &Caller, copy: &Copy) -> Result<(), Failure> {
    sync::permitted(node, copy).map_err(failure)?;
    let link = node.link_of(&copy.account.hub).ok_or_else(|| failure("the replica's hub is unavailable"))?;
    let view = link.view();
    if !view.shared_records {
        return Err(Failure::not_run("update the hub before editing replicated plans or history"));
    }
    let acting = crate::around::acting_of(node, caller);
    let allowed = node.accounts_on(&link, &acting).iter().any(|account| match &copy.head.document {
        Document::History { scope: Scope::Project { town, .. }, .. } => view.in_project(account, town),
        Document::History { scope: Scope::Conversation { conversation }, .. } => fastctx_net::record_ownership::allows(&view.directory, account, std::slice::from_ref(&conversation.account), conversation.shared_on.as_ref()),
        Document::Plan { owner } => node.replicas.state.folders.lock().expect("never poisoned").values().any(|folder| {
            folder.account.hub == copy.account.hub && matches!(&folder.scope, Scope::Conversation { conversation } if &conversation.address == owner && fastctx_net::record_ownership::allows(&view.directory, account, std::slice::from_ref(&conversation.account), conversation.shared_on.as_ref()))
        }),
    });
    if allowed { Ok(()) } else { Err(Failure::refused("this conversation cannot edit that record")) }
}

pub(crate) async fn intercept(
    node: &Arc<Node>,
    caller: &Caller,
    operation: &WriteOp,
    job: Option<&JobId>,
    town: Option<&TownName>,
) -> Result<Option<WriteOutcome>, Failure> {
    let paths = fastctx_hands::write::touched_paths(operation).map_err(failure)?;
    if matches!(operation, WriteOp::ApplyPatch(_)) {
        for path in paths {
            if locate(node, &path).await?.is_some() {
                return Ok(Some(Err(
                    "plans and history are changed with write or edit; no part of this patch was applied"
                        .to_string()
                        .into(),
                )));
            }
        }
        return Ok(None);
    }
    let Some(path) = paths.first() else { return Ok(None) };
    let Some(slot) = locate(node, path).await? else { return Ok(None) };
    let author = crate::history::api::author(node, caller).await?;
    let mut copy = slot.lock().await;
    if matches!(copy.head.document, Document::History { .. }) {
        crate::history::offered::check(node, caller)?;
    }
    authorize(node, caller, &copy)?;
    if copy.head.retired {
        return Ok(Some(Err("this document was retired; restore it in the app before editing".to_string().into())));
    }
    super::publication::repair(node, &mut copy).await.map_err(failure)?;
    let previous = sync::text(node, &copy, copy.effective()).await.map_err(failure)?;
    let link = node.link_of(&copy.account.hub).ok_or_else(|| failure("the replica's hub is unavailable"))?;
    let offline = !matches!(link.status(), fastctx_net::link::Status::Online { .. }) || !link.view_current();
    let mut base = copy.head.version;
    let mut before = previous.clone();
    if matches!(operation, WriteOp::Write(_))
        && let Caller::Resident(owner) = caller
    {
        match copy.observed.get(&owner.key()) {
            // A history document is shared: replacing one unseen would drop
            // what others wrote in it since.
            None if matches!(copy.head.document, Document::History { .. }) && !previous.is_empty() => {
                return Ok(Some(Err("write replaces this whole history document; view it with fastctx view \
                     before replacing it, or change part of it with edit"
                    .to_string()
                    .into())));
            }
            None => {}
            Some(observed) => {
                let expected = observed
                    .hash
                    .as_ref()
                    .ok_or_else(|| failure("the file changed while it was read; view it again before writing"))?;
                if copy.effective().hash != *expected {
                    base = observed.version;
                    if offline {
                        return Ok(Some(Err(
                            "this local copy changed since it was viewed; read it again before writing offline"
                                .to_string()
                                .into(),
                        )));
                    }
                    before = super::backend::base(&link, &copy, base).await.map_err(failure)?;
                    // What was seen held edits made offline that reached the hub
                    // since: the revision under them is the common base.
                    if Text::of(&before).hash != *expected && !observed.local {
                        return Err(failure("the observed base is unavailable; view the latest copy before writing"));
                    }
                }
            }
        }
    }
    let bytes = fastctx_hands::write::candidate::prepare(operation, Some(before.as_bytes())).map_err(failure)?;
    let text = String::from_utf8(bytes).map_err(|_| failure("plans and history use UTF-8"))?;
    match &copy.head.document {
        Document::Plan { .. } => fastctx_types::plan::validate(&text).map_err(failure)?,
        Document::History { name, .. } => {
            history::markdown::parse_written(name, &text).map_err(failure)?;
        }
    }
    if text == previous && !matches!(operation, WriteOp::Write(_)) {
        return Ok(Some(Ok(WriteResult {
            note_prefix: 0,
            text: "No change.".into(),
            changes: Vec::new(),
            changed_since_seen: Vec::new(),
        })));
    }
    if copy.pending.len() >= WAITING_EDITS {
        return Ok(Some(Err(format!(
            "{WAITING_EDITS} edits of this document are waiting for its hub; connect before editing it again"
        )
        .into())));
    }
    let id = fastctx_net::sign::random::<32>().map_err(failure)?.iter().map(|byte| format!("{byte:02x}")).collect();
    // The edit lands on a copy of the state, kept only once it is saved.
    let mut next = copy.clone();
    let edit = Pending {
        id,
        base,
        before: sync::keep(node, &next, &before).await.map_err(failure)?,
        text: sync::keep(node, &next, &text).await.map_err(failure)?,
        edit: (!matches!(operation, WriteOp::Write(_))).then(|| operation.clone()),
        author,
        offline,
    };
    let edit_id = edit.id.clone();
    next.pending.push(edit);
    let submitted = if offline { Ok(()) } else { flush(node, &link, &mut next).await };
    if submitted.is_err() {
        for pending in &mut next.pending {
            pending.offline = true;
        }
    }
    // An edit still waiting reaches the hub later; one the hub refused is gone.
    let waiting = next.pending.iter().any(|pending| pending.id == edit_id);
    if let Err(error) = &submitted
        && waiting
    {
        node.log.line(format_args!("keeping an edit of {} for its hub: {error}", next.path.display()));
    }
    let after = next.effective().clone();
    if let Caller::Resident(owner) = caller {
        let local = next.pending.iter().any(|pending| pending.offline);
        next.observed
            .insert(owner.key(), Observed { version: next.head.version, hash: Some(after.hash.clone()), local });
    }
    let shown = copy.effective().clone();
    super::publication::save(node, &mut next, true, Some(&shown)).await.map_err(failure)?;
    *copy = next;
    let after = sync::text(node, &copy, &after).await.map_err(failure)?;
    let path = copy.path.clone();
    let document = copy.head.document.clone();
    drop(copy);
    crate::context::invalidations::document(node, link.hub(), &document);
    node.memory_changed();
    let text = if waiting {
        format!("{} {}: saved locally; the hub will receive it when connected", operation.tool(), path.display())
    } else {
        format!("{} {}: saved and synchronized", operation.tool(), path.display())
    };
    let result = WriteResult {
        note_prefix: text.len(),
        text,
        changes: vec![FileChange {
            fact: None,
            path: path.clone(),
            kind: if base.is_some() { ChangeKind::Modified } else { ChangeKind::Added },
            before_hash: Some(fastctx_hands::files::content_hash(previous.as_bytes())),
            after_hash: Some(fastctx_hands::files::content_hash(after.as_bytes())),
            encoding: Some("utf-8".into()),
            encoding_selection: None,
        }],
        changed_since_seen: Vec::new(),
    };
    let result = if let Caller::Resident(owner) = caller {
        let (world, names, owner, job, town) =
            (node.world.clone(), node.names(), owner.clone(), job.cloned(), town.cloned());
        fastctx_platform::task::spawn_blocking(move || {
            crate::write::record::record(
                &world,
                &names,
                &owner,
                &path,
                Some(previous.as_bytes()),
                after.as_bytes(),
                job.as_ref(),
                town.as_ref(),
                result,
            )
        })
        .await
        .map_err(failure)?
        .map_err(failure)?
    } else {
        result
    };
    Ok(Some(match submitted {
        Err(error) if !waiting => Err(WriteError { note_prefix: 0, message: error, changes: result.changes }),
        _ => Ok(result),
    }))
}

/// Sends the copy's waiting edits to its hub, oldest first. It changes the
/// copy in memory only: its caller keeps it, whether or not every edit went.
pub(crate) async fn flush(node: &Arc<Node>, link: &fastctx_net::link::Link, copy: &mut Copy) -> Result<(), String> {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(55);
    while let Some(pending) = copy.pending.first().cloned() {
        sync::permitted(node, copy)?;
        if super::backend::accepted(link, copy, &pending).await? {
            copy.pending.remove(0);
            sync::fetch(node, link, copy).await?;
            continue;
        }
        // A new history name has no body yet; a catalog distinguishes absence from a failed read.
        let missing = if copy.head.version.is_none() && matches!(copy.head.document, Document::History { .. }) {
            match &copy.head.document {
                Document::History { scope: Scope::Project { town, .. }, name } => !link
                    .history_catalog(&copy.account.account, town)
                    .await?
                    .documents
                    .iter()
                    .any(|revision| revision.name == *name),
                Document::History { scope: Scope::Conversation { conversation }, name } => !link
                    .conversation_history_catalog(&copy.account.account, &conversation.address)
                    .await?
                    .documents
                    .iter()
                    .any(|revision| revision.name == *name),
                _ => false,
            }
        } else {
            false
        };
        if !missing {
            sync::fetch(node, link, copy).await?;
        }
        if copy.head.retired && !pending.offline {
            return reject(copy, "the document was retired; restore it in the app before editing");
        }
        if matches!(copy.head.document, Document::Plan { .. }) && copy.head.retired {
            return reject(copy, "this conversation's plan was deleted");
        }
        let hub = sync::text(node, copy, &copy.body.clone()).await?;
        let written = sync::text(node, copy, &pending.text).await?;
        let merged = match &pending.edit {
            None if pending.base.is_none()
                && pending.before.len == 0
                && copy.head.version.is_some()
                && matches!(copy.head.document, Document::History { .. }) =>
            {
                None
            }
            None => fastctx_net::plan_merge::try_merge(&sync::text(node, copy, &pending.before).await?, &hub, &written),
            Some(edit) => fastctx_hands::write::candidate::prepare(edit, Some(hub.as_bytes()))
                .ok()
                .and_then(|bytes| String::from_utf8(bytes).ok()),
        };
        let mut text = match merged {
            Some(text) => text,
            None if pending.offline => written.clone(),
            None => {
                return reject(
                    copy,
                    "the document changed elsewhere and these edits conflict; read the latest copy before editing again",
                );
            }
        };
        let valid = match &copy.head.document {
            Document::Plan { .. } => fastctx_types::plan::validate(&text),
            Document::History { name, .. } => history::markdown::parse_written(name, &text).map(|_| ()),
        };
        if let Err(error) = valid {
            if pending.offline {
                text = written;
            } else {
                return reject(copy, &error);
            }
        }
        sync::permitted(node, copy)?;
        match super::backend::submit(link, copy, &pending, text).await? {
            Some(taken) => {
                copy.pending.remove(0);
                copy.body = sync::keep(node, copy, &taken).await?;
            }
            None if tokio::time::Instant::now() >= deadline => {
                return Err("the document keeps changing; this edit remains pending".into());
            }
            None => {}
        }
    }
    Ok(())
}

/// Drops the first waiting edit, which the hub will not take.
fn reject(copy: &mut Copy, reason: &str) -> Result<(), String> {
    copy.pending.remove(0);
    Err(reason.into())
}

pub(crate) async fn saw(
    node: &Arc<Node>,
    owner: &Address,
    seen: &[fastctx_types::residents::Seen],
) -> Result<(), Failure> {
    node.replicas.state.load(&node.world).map_err(failure)?;
    for seen in seen {
        let Some(slot) = node.replicas.state.at(Path::new(&seen.path)) else { continue };
        let mut copy = slot.lock().await;
        let shown = copy.effective().clone();
        let same = seen.sha256.as_ref().map_or_else(
            || {
                fastctx_platform::disk::metadata(&copy.path).ok().is_some_and(|metadata| {
                    metadata.len() == seen.size
                        && metadata.modified().ok().and_then(|time| jiff::Timestamp::try_from(time).ok())
                            == seen.modified
                })
            },
            |seen| *seen == shown.hash,
        );
        let local = copy.pending.iter().any(|pending| pending.offline);
        let observed = Observed { version: copy.head.version, hash: same.then_some(shown.hash), local };
        if copy.observed.get(&owner.key()) == Some(&observed) {
            continue;
        }
        copy.observed.insert(owner.key(), observed);
        super::publication::save(node, &mut copy, false, None).await.map_err(failure)?;
    }
    Ok(())
}
