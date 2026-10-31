//! Standalone history uses the same controlled writes and read-only projections.

use crate::{Node, town::caller::Caller};
use fastctx_types::{
    Address, JobId, TownName,
    history::{HistoryCall, HistoryReply, Scope},
    ipc::Failure,
    write::{ChangeKind, FileChange, WriteOp, WriteOutcome, WriteResult},
};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

fn failure(error: impl std::fmt::Display) -> Failure {
    Failure::new(error.to_string())
}

pub(crate) fn folder(node: &Node, scope: &Scope) -> PathBuf {
    match scope {
        Scope::Project { project } => node.world.town(project).path().join("history"),
        Scope::Conversation { resident, machine } => node
            .world
            .owner(&node.names(), &Address { resident: resident.clone(), machine: machine.clone() })
            .path()
            .join("history"),
    }
}

fn locate(node: &Node, path: &Path) -> Result<Option<(Scope, String)>, Failure> {
    if !node.links().is_empty() {
        return Ok(None);
    }
    let path = fastctx_platform::local_path(path).map_err(failure)?;
    let Some(name) = path.file_name().and_then(|name| name.to_str()).and_then(|name| name.strip_suffix(".md")) else {
        return Ok(None);
    };
    let Some(parent) = path.parent() else { return Ok(None) };
    let mut scopes =
        node.world.towns().map_err(failure)?.into_iter().map(|project| Scope::Project { project }).collect::<Vec<_>>();
    scopes.extend(
        node.residents
            .everyone()
            .filter(|resident| resident.removed.is_none())
            .map(|resident| Scope::Conversation { resident: resident.id, machine: node.machine() }),
    );
    for scope in scopes {
        if crate::replicas::state::path_key(&folder(node, &scope)) == crate::replicas::state::path_key(parent) {
            fastctx_types::history::markdown::check_name(name).map_err(failure)?;
            if crate::replicas::state::path_key(&fastctx_hands::files::canonical_path(&path).map_err(failure)?)
                != crate::replicas::state::path_key(&path)
            {
                return Err(Failure::refused("the history file was redirected; restore it before editing"));
            }
            return Ok(Some((scope, name.into())));
        }
    }
    Ok(None)
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
            if locate(node, &path)?.is_some() {
                return Ok(Some(Err("history is changed with write or edit; no part of this patch was applied"
                    .to_string()
                    .into())));
            }
        }
        return Ok(None);
    }
    let Some(path) = paths.first() else { return Ok(None) };
    let Some((scope, name)) = locate(node, path)? else { return Ok(None) };
    if !fastctx_platform::preview() {
        return Err(Failure::not_run("history is not enabled in this release"));
    }
    let author = super::api::author(node, caller).await?;
    super::api::network(node, caller, &scope).await?;
    let latest = super::local::call(node, HistoryCall::List { scope: scope.clone() }, None).await?;
    let HistoryReply::Catalog { catalog } = latest else {
        return Err(failure("the local history catalog was not returned"));
    };
    let revision = catalog.documents.iter().find(|revision| revision.name == name);
    if revision.is_some_and(|revision| revision.retired) {
        return Err(failure("restore this document in the app before editing"));
    }
    let document = if revision.is_some() {
        let HistoryReply::Document { document } = super::local::call(
            node,
            HistoryCall::Read { scope: scope.clone(), name: name.clone(), version: None },
            None,
        )
        .await?
        else {
            return Err(failure("the local history document was not returned"));
        };
        Some(document)
    } else {
        None
    };
    let previous = document.as_ref().map_or("", |document| document.text.as_str()).to_owned();
    let mut before = previous.clone();
    if matches!(operation, WriteOp::Write(_))
        && let Caller::Resident(owner) = caller
    {
        let seen = crate::write::load_seen(&node.world.owner(&node.names(), owner).seen()).map_err(failure)?;
        let canonical = fastctx_hands::files::canonical_path(path).map_err(failure)?;
        if let Some(seen) = seen.get(&canonical)
            && let fastctx_types::files::SeenVersion::Full { sha256 } = &seen.version
            && fastctx_hands::files::content_hash(before.as_bytes()) != *sha256
        {
            let mut found = None;
            if let Some(document) = &document {
                for version in document.versions.iter().rev() {
                    if let HistoryReply::Document { document } = super::local::call(
                        node,
                        HistoryCall::Read { scope: scope.clone(), name: name.clone(), version: Some(version.version) },
                        None,
                    )
                    .await?
                        && fastctx_hands::files::content_hash(document.text.as_bytes()) == *sha256
                    {
                        found = Some(document.text);
                        break;
                    }
                }
            }
            before = found.ok_or_else(|| failure("the observed history version is unavailable; read it again"))?;
        }
    }
    let proposed = fastctx_hands::write::candidate::prepare(operation, document.as_ref().map(|_| before.as_bytes()))
        .map_err(failure)?;
    let proposed = String::from_utf8(proposed).map_err(failure)?;
    let mut current = previous.clone();
    let mut expected = revision.map(|revision| revision.version);
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(55);
    let after = loop {
        let text = if matches!(operation, WriteOp::Write(_)) {
            fastctx_net::plan_merge::try_merge(&before, &current, &proposed)
                .ok_or_else(|| failure("the history changed elsewhere and these edits conflict; read it again"))?
        } else {
            String::from_utf8(
                fastctx_hands::write::candidate::prepare(operation, Some(current.as_bytes())).map_err(failure)?,
            )
            .map_err(failure)?
        };
        match super::local::call(
            node,
            HistoryCall::Save { scope: scope.clone(), name: name.clone(), text: text.clone(), expected },
            Some(author.clone()),
        )
        .await
        {
            Ok(_) => break text,
            Err(error)
                if error.message.starts_with("the document changed;") && tokio::time::Instant::now() < deadline =>
            {
                let HistoryReply::Document { document } = super::local::call(
                    node,
                    HistoryCall::Read { scope: scope.clone(), name: name.clone(), version: None },
                    None,
                )
                .await?
                else {
                    return Err(error);
                };
                current = document.text;
                expected = Some(document.revision.version);
            }
            Err(error) => return Err(error),
        }
    };
    let note = format!("{} {}: saved", operation.tool(), path.display());
    let mut result = WriteResult {
        note_prefix: note.len(),
        text: note,
        changes: vec![FileChange {
            fact: None,
            path: path.clone(),
            kind: if revision.is_some() { ChangeKind::Modified } else { ChangeKind::Added },
            before_hash: Some(fastctx_hands::files::content_hash(previous.as_bytes())),
            after_hash: Some(fastctx_hands::files::content_hash(after.as_bytes())),
            encoding: Some("utf-8".into()),
            encoding_selection: None,
        }],
        changed_since_seen: Vec::new(),
    };
    if let Caller::Resident(owner) = caller {
        result = crate::write::record::record(
            &node.world,
            &node.names(),
            owner,
            path,
            Some(previous.as_bytes()),
            after.as_bytes(),
            job,
            town,
            result,
        )
        .map_err(failure)?;
    }
    Ok(Some(Ok(result)))
}

pub(crate) async fn repair(node: &Arc<Node>) -> Result<(), Failure> {
    if !node.links().is_empty() || !fastctx_platform::preview() {
        return Ok(());
    }
    let mut scopes =
        node.world.towns().map_err(failure)?.into_iter().map(|project| Scope::Project { project }).collect::<Vec<_>>();
    scopes.extend(
        node.residents
            .everyone()
            .filter(|resident| resident.removed.is_none())
            .map(|resident| Scope::Conversation { resident: resident.id, machine: node.machine() }),
    );
    for scope in scopes {
        super::local::call(node, HistoryCall::List { scope }, None).await?;
    }
    Ok(())
}
