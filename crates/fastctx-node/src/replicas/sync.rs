//! Event-driven discovery and per-document refresh; no filesystem watching.

use super::state::{self, Copy, Folder};
use crate::Node;
use fastctx_net::link::Link;
use fastctx_types::{
    access::HubAccount,
    owned_records::{Request, Response},
    replicas::{Document, Head, Scope},
};
use std::{path::PathBuf, sync::Arc};

fn failure(error: impl std::fmt::Display) -> String {
    error.to_string()
}

pub(crate) fn path(node: &Node, link: &Link, document: &Document) -> Result<PathBuf, String> {
    Ok(match document {
        Document::Plan { owner } => node.world.owner(&node.names(), owner).path().join("plan.md"),
        Document::History { scope, name } => {
            fastctx_types::history::markdown::check_name(name)?;
            folder(node, link, scope)?.join(format!("{name}.md"))
        }
    })
}

pub(crate) fn folder(node: &Node, link: &Link, scope: &Scope) -> Result<PathBuf, String> {
    Ok(match scope {
        Scope::Project { town, .. } => {
            node.world.town(&crate::ledger::local_name(node, link, town)?).path().join("history")
        }
        Scope::Conversation { conversation } => {
            node.world.owner(&node.names(), &conversation.address).path().join("history")
        }
    })
}

pub(crate) fn permitted(node: &Node, copy: &Copy) -> Result<(), String> {
    super::publication::permitted(node, &state::Route::from(copy))
}

pub(crate) async fn all(node: &Arc<Node>, link: &Arc<Link>) -> Result<(), String> {
    if !link.view().shared_records {
        return Ok(());
    }
    node.replicas.state.load(&node.world).map_err(failure)?;
    let view = crate::authorization::current_view(link).map_err(failure)?;
    let Some(account) = view
        .me()
        .filter(|member| !member.managed && member.is_machine())
        .and_then(|member| member.home.as_ref().filter(|home| member.logins.contains(home)))
        .cloned()
    else {
        return super::publication::prune(node);
    };
    for resident in node.residents.everyone().filter(|resident| resident.removed.is_none()) {
        if let Err(error) = crate::record_ownership::register(node, &resident.id, &resident.about.acting).await {
            node.log.line(format_args!("registering the records of {}: {error}", resident.id));
        }
    }
    let known: Vec<_> = node
        .replicas
        .state
        .routes
        .lock()
        .expect("never poisoned")
        .iter()
        .map(|(id, route)| (id.clone(), route.clone()))
        .collect();
    let known_folders: Vec<_> = node.replicas.state.folders.lock().expect("never poisoned").values().cloned().collect();
    let Response::Replicas { catalog } = link.owned_records(&account, Request::Replicas).await? else {
        return Err("the hub did not return the replica catalog".into());
    };
    let account = HubAccount { hub: link.membership().id, account };
    super::publication::prune(node)?;
    for scope in catalog.scopes.iter().cloned() {
        let path = folder(node, link, &scope)?;
        if let Scope::Conversation { conversation } = &scope {
            let owner = HubAccount { hub: account.hub.clone(), account: conversation.account.clone() };
            crate::placement::ownership::bind_registered(
                node,
                path.parent().ok_or("history has no conversation directory")?,
                &owner,
                conversation,
            )
            .map_err(failure)?;
        }
        super::publication::folder(node, Folder { account: account.clone(), scope, path })?;
    }
    for head in catalog.documents.iter().cloned() {
        apply(node, link, &account, head).await?;
    }
    super::publication::catalog(node, &account.hub, &catalog, known, known_folders)?;
    super::publication::prune(node)?;
    node.memory_changed();
    Ok(())
}

pub(crate) async fn apply(node: &Arc<Node>, link: &Arc<Link>, account: &HubAccount, head: Head) -> Result<(), String> {
    if let Document::Plan { owner } = &head.document {
        crate::placement::copies::upgrade::take(node, link, &head.owner, owner).await.map_err(failure)?;
    }
    let path = path(node, link, &head.document)?;
    let id = state::identity(account, &head);
    let existing = node.replicas.state.get(&id);
    let known = existing.is_some();
    let slot = existing.unwrap_or_else(|| {
        node.replicas.state.insert(Copy {
            epoch: 0,
            account: account.clone(),
            head: Head { version: None, ..head.clone() },
            path: path.clone(),
            text: String::new(),
            observed: Default::default(),
            pending: Vec::new(),
        })
    });
    let mut copy = slot.lock().await;
    if known && copy.path == path && copy.head == head && copy.pending.is_empty() {
        return super::publication::repair(node, &copy);
    }
    let old_path = copy.path.clone();
    copy.path = path;
    copy.head.document = head.document.clone();
    permitted(node, &copy)?;
    if head.retired && copy.pending.is_empty() {
        copy.head = head;
        copy.text.clear();
    } else if copy.head.version != head.version || copy.head.retired != head.retired || !copy.pending.is_empty() {
        refresh(node, link, &mut copy).await?;
    } else {
        copy.head = head;
    }
    if !copy.pending.is_empty() {
        super::writing::flush(node, link, &mut copy).await?;
    }
    if copy.head.retired && copy.pending.is_empty() {
        copy.text.clear();
    }
    super::publication::save(node, &copy, true, Some(copy.effective()))?;
    if old_path != copy.path {
        super::files::withdraw(&old_path).map_err(failure)?;
    }
    Ok(())
}

pub(crate) async fn refresh(node: &Arc<Node>, link: &Link, copy: &mut Copy) -> Result<(), String> {
    let previous = copy.effective().to_string();
    let (text, version, retired, history) = match &copy.head.document {
        Document::Plan { owner } => {
            let page = link.owned_plan_read(&copy.account.account, owner).await?;
            (
                page.body.as_ref().map_or(String::new(), |body| body.markdown.clone()),
                page.version(),
                page.deleted(),
                None,
            )
        }
        Document::History { scope: Scope::Project { town, .. }, name } => {
            let document = link.history_current(&copy.account.account, town, name).await?;
            (document.text, Some(document.revision.version), document.revision.retired, Some(document.revision))
        }
        Document::History { scope: Scope::Conversation { conversation }, name } => {
            let stored =
                link.conversation_history_stored(&copy.account.account, &conversation.address, name, None).await?;
            match stored {
                Some(stored) => {
                    (stored.text, Some(stored.revision.version), stored.revision.retired, Some(stored.revision))
                }
                None => (String::new(), None, false, None),
            }
        }
    };
    permitted(node, copy)?;
    copy.text = if retired && copy.pending.is_empty() { String::new() } else { text };
    copy.head.version = version;
    copy.head.retired = retired;
    copy.head.history = history;
    super::publication::save(node, copy, true, Some(&previous))?;
    Ok(())
}

pub(crate) async fn repair(node: &Arc<Node>) -> Result<(), String> {
    node.replicas.repairs.run(|| repair_now(node.clone())).await
}

async fn repair_now(node: Arc<Node>) -> Result<(), String> {
    if node.links().is_empty() {
        crate::history::local_copy::repair(&node).await.map_err(|error| error.message)?;
    }
    fastctx_platform::task::spawn_blocking(move || {
        super::publication::prune(&node)?;
        for slot in node.replicas.state.slots() {
            // A writer repairs its own projection before returning; other documents never wait for it.
            let Ok(copy) = slot.try_lock() else { continue };
            if permitted(&node, &copy).is_ok() {
                super::publication::repair(&node, &copy)?;
            }
        }
        Ok(())
    })
    .await
    .map_err(|error| error.to_string())?
}

pub(crate) async fn project(
    node: &Arc<Node>,
    link: &Arc<Link>,
    town: &fastctx_types::access::TownId,
) -> Result<(), String> {
    if !link.view().shared_records {
        return Ok(());
    }
    node.replicas.state.load(&node.world).map_err(failure)?;
    let folders: Vec<_> = node
        .replicas
        .state
        .folders
        .lock()
        .expect("never poisoned")
        .values()
        .filter(|folder| {
            folder.account.hub == link.membership().id
                && matches!(&folder.scope, Scope::Project { town: id, .. } if id == town)
        })
        .cloned()
        .collect();
    if folders.is_empty() {
        return all(node, link).await;
    }
    for folder in folders {
        let view = crate::authorization::current_view(link).map_err(failure)?;
        if !view.member_keeps_project(&view.me, town) {
            return all(node, link).await;
        }
        let project = view.project(town).ok_or("the project was removed")?;
        let owner = project.owner.clone();
        let opened = view.account(&owner).ok_or("the project's account is unavailable")?.opened;
        let catalog = link.history_catalog(&folder.account.account, town).await?;
        for revision in catalog.documents {
            let head = Head {
                owner: owner.clone(),
                opened,
                document: Document::History { scope: folder.scope.clone(), name: revision.name.clone() },
                version: Some(revision.version),
                retired: revision.retired,
                history: Some(revision),
            };
            apply(node, link, &folder.account, head).await?;
        }
    }
    node.memory_changed();
    Ok(())
}

pub(crate) async fn record(
    node: &Arc<Node>,
    link: &Arc<Link>,
    change: &fastctx_types::account_records::Changed,
) -> Result<(), String> {
    if !link.view().shared_records {
        return Ok(());
    }
    node.replicas.state.load(&node.world).map_err(failure)?;
    let resident = if change.metadata.kind == fastctx_types::plan::KIND {
        change.metadata.key.as_str()
    } else {
        change.metadata.key.split_once('/').map(|(resident, _)| resident).ok_or("invalid history record key")?
    };
    let folders: Vec<_> = node.replicas.state.folders.lock().expect("never poisoned").values()
        .filter(|folder| folder.account.hub == link.membership().id && matches!(&folder.scope, Scope::Conversation { conversation } if conversation.account == change.account && conversation.address.resident.as_str() == resident)).cloned().collect();
    if folders.is_empty() {
        return all(node, link).await;
    }
    for folder in folders {
        let Scope::Conversation { conversation } = &folder.scope else { unreachable!() };
        let view = crate::authorization::current_view(link).map_err(failure)?;
        let Some(owner) = view.account(&conversation.account) else { return all(node, link).await };
        let document = if change.metadata.kind == fastctx_types::plan::KIND {
            Document::Plan { owner: conversation.address.clone() }
        } else {
            Document::History {
                scope: folder.scope.clone(),
                name: change.metadata.key.split_once('/').expect("checked key").1.into(),
            }
        };
        let head = Head {
            owner: conversation.account.clone(),
            opened: owner.opened,
            document,
            version: Some(change.metadata.version),
            retired: change.metadata.deleted,
            history: None,
        };
        apply(node, link, &folder.account, head).await?;
    }
    node.memory_changed();
    Ok(())
}

pub(crate) async fn repair_paths(node: &Arc<Node>, paths: &[PathBuf]) -> Result<(), String> {
    if node.links().is_empty() {
        crate::history::local_copy::repair(node).await.map_err(|error| error.message)?;
    }
    node.replicas.state.load(&node.world).map_err(failure)?;
    for path in paths {
        let native = fastctx_platform::local_path(path).map_err(failure)?;
        let actual = fastctx_hands::files::canonical_path(&native).map_err(failure)?;
        let Some(slot) = node.replicas.state.at(&native).or_else(|| node.replicas.state.at(&actual)) else { continue };
        // A document being written may be waiting on the hub: its reader gets
        // the copy as it stands rather than waiting with it (the writer
        // repairs and publishes its own projection when it is done).
        let Ok(copy) = tokio::time::timeout(READ_WAIT, slot.lock()).await else { continue };
        super::publication::repair(node, &copy)?;
    }
    Ok(())
}

/// How long a read waits for a document's writer before reading the copy
/// as it stands.
const READ_WAIT: std::time::Duration = std::time::Duration::from_secs(2);

/// Preparing one newly opened conversation does not enumerate every other conversation.
pub(crate) async fn conversation(
    node: &Arc<Node>,
    link: &Arc<Link>,
    conversation: fastctx_types::owned_records::Conversation,
) -> Result<(), String> {
    node.replicas.state.load(&node.world).map_err(failure)?;
    let view = crate::authorization::current_view(link).map_err(failure)?;
    let account = HubAccount {
        hub: link.membership().id,
        account: view.me().and_then(|member| member.home.clone()).ok_or("this computer has no registered account")?,
    };
    let opened = view.account(&conversation.account).ok_or("the conversation account is unavailable")?.opened;
    let scope = Scope::Conversation { conversation: conversation.clone() };
    let path = folder(node, link, &scope)?;
    crate::placement::ownership::bind_registered(
        node,
        path.parent().ok_or("history has no conversation directory")?,
        &HubAccount { hub: account.hub.clone(), account: conversation.account.clone() },
        &conversation,
    )
    .map_err(failure)?;
    super::publication::folder(node, Folder { account: account.clone(), scope: scope.clone(), path })?;
    let page = link.owned_plan_read(&account.account, &conversation.address).await?;
    apply(
        node,
        link,
        &account,
        Head {
            owner: conversation.account.clone(),
            opened,
            document: Document::Plan { owner: conversation.address.clone() },
            version: page.version(),
            retired: page.deleted(),
            history: None,
        },
    )
    .await?;
    let catalog = link.conversation_history_catalog(&account.account, &conversation.address).await?;
    for revision in catalog.documents {
        apply(
            node,
            link,
            &account,
            Head {
                owner: conversation.account.clone(),
                opened,
                document: Document::History { scope: scope.clone(), name: revision.name.clone() },
                version: Some(revision.version),
                retired: revision.retired,
                history: Some(revision),
            },
        )
        .await?;
    }
    Ok(())
}

/// A topic invalidation carries only its identity and revision, never the whole catalog.
pub(crate) async fn topic(
    node: &Arc<Node>,
    link: &Arc<Link>,
    town: &fastctx_types::access::TownId,
    name: &str,
    version: u64,
) -> Result<(), String> {
    let view = crate::authorization::current_view(link).map_err(failure)?;
    if !view.member_keeps_project(&view.me, town) {
        return Ok(());
    }
    let project = view.project(town).ok_or("the history project is unavailable")?;
    let scope = Scope::Project { town: town.clone(), name: project.name.clone() };
    let folder = folder(node, link, &scope)?;
    let account =
        node.replicas.state.folders.lock().expect("never poisoned").get(&folder).map(|folder| folder.account.clone());
    let Some(account) = account else {
        return all(node, link).await;
    };
    let opened = view.account(&project.owner).ok_or("the project's owning account is unavailable")?.opened;
    apply(
        node,
        link,
        &account,
        Head {
            owner: project.owner.clone(),
            opened,
            document: Document::History { scope, name: name.into() },
            version: Some(version),
            retired: false,
            history: None,
        },
    )
    .await
}
