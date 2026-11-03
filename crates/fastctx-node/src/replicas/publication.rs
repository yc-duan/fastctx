//! Short local publication transactions never wait for a document's network operation.

use super::state::{self, Copy, Folder, Route};
use crate::Node;
use fastctx_types::replicas::{Document, Scope};
use std::sync::Arc;

fn scope_allowed(node: &Node, folder: &Folder) -> bool {
    let Some(link) = node.link_of(&folder.account.hub) else { return false };
    let view = link.view();
    let Some(member) = view.me() else { return false };
    match &folder.scope {
        Scope::Project { town, .. } => view.member_keeps_project(&view.me, town),
        Scope::Conversation { conversation } => fastctx_net::record_ownership::keeps(
            &view.directory,
            member,
            std::slice::from_ref(&conversation.account),
            conversation.shared_on.as_ref(),
        ),
    }
}

pub(crate) fn permitted(node: &Node, route: &Route) -> Result<(), String> {
    let link = node.link_of(&route.account.hub).ok_or("the replica's account left this computer")?;
    let view = link.view();
    if view.account(&route.head.owner).is_none_or(|owner| owner.opened != route.head.opened) {
        return Err("the replica's owning account was replaced or removed".into());
    }
    let allowed = match &route.head.document {
        Document::History { scope, .. } => {
            node.replicas.state.folders.lock().expect("never poisoned").values().any(|folder| {
                folder.account.hub == route.account.hub && state::scope_key(&folder.scope) == state::scope_key(scope)
            }) && scope_allowed(
                node,
                &Folder { account: route.account.clone(), scope: scope.clone(), path: route.path.clone() },
            )
        }
        Document::Plan { owner } => {
            node.replicas.state.folders.lock().expect("never poisoned").values().any(|folder| {
                folder.account.hub == route.account.hub
                    && matches!(&folder.scope, Scope::Conversation { conversation } if &conversation.address == owner)
                    && scope_allowed(node, folder)
            })
        }
    };
    if allowed { Ok(()) } else { Err("this computer no longer keeps this record".into()) }
}

/// `save_now`, where writing the disk may block.
pub(crate) async fn save(node: &Arc<Node>, copy: &Copy, publish: bool, previous: Option<&str>) -> Result<(), String> {
    let (node, copy, previous) = (node.clone(), copy.clone(), previous.map(str::to_owned));
    fastctx_platform::task::blocking(move || save_now(&node, &copy, publish, previous.as_deref())).await
}

/// Checks again inside the same local transaction used by revocation.
pub(crate) fn save_now(node: &Node, copy: &Copy, publish: bool, previous: Option<&str>) -> Result<(), String> {
    let _publication = node.replicas.state.publication.read().expect("never poisoned");
    let route = Route::from(copy);
    permitted(node, &route)?;
    if node
        .replicas
        .state
        .routes
        .lock()
        .expect("never poisoned")
        .get(&copy.id())
        .is_none_or(|current| current.epoch != copy.epoch)
    {
        return Err("this document copy was withdrawn while its operation was in progress".into());
    }
    let changed = node
        .replicas
        .state
        .routes
        .lock()
        .expect("never poisoned")
        .get(&copy.id())
        .is_none_or(|before| before.head != route.head || before.path != route.path);
    node.replicas.state.relocated(copy);
    state::save(&node.world, copy).map_err(|error| error.to_string())?;
    node.replicas.state.routes.lock().expect("never poisoned").insert(copy.id(), route);
    if publish {
        project(copy, previous)?;
    }
    drop(_publication);
    if changed {
        crate::context::invalidations::document(node, &copy.account.hub, &copy.head.document);
    }
    Ok(())
}

fn project(copy: &Copy, previous: Option<&str>) -> Result<(), String> {
    if matches!(copy.head.document, Document::History { .. }) && copy.head.version.is_none() && copy.pending.is_empty()
    {
        return Ok(());
    }
    if copy.head.retired && copy.pending.is_empty() {
        super::files::delete(&copy.path)
    } else {
        super::files::publish(&copy.path, copy.effective(), previous)
    }
    .map_err(|error| error.to_string())
}

/// `repair_now`, where writing the disk may block.
pub(crate) async fn repair(node: &Arc<Node>, copy: &Copy) -> Result<(), String> {
    let (node, copy) = (node.clone(), copy.clone());
    fastctx_platform::task::blocking(move || repair_now(&node, &copy)).await
}

pub(crate) fn repair_now(node: &Node, copy: &Copy) -> Result<(), String> {
    if node.link_of(&copy.account.hub).is_some_and(|link| !link.view().shared_records) {
        return Ok(());
    }
    let _publication = node.replicas.state.publication.read().expect("never poisoned");
    permitted(node, &Route::from(copy))?;
    project(copy, Some(copy.effective()))
}

/// `folder_now`, where writing the disk may block.
pub(crate) async fn folder(node: &Arc<Node>, folder: Folder) -> Result<(), String> {
    let node = node.clone();
    fastctx_platform::task::blocking(move || folder_now(&node, folder)).await
}

pub(crate) fn folder_now(node: &Node, folder: Folder) -> Result<(), String> {
    let _publication = node.replicas.state.publication.write().expect("never poisoned");
    if !scope_allowed(node, &folder) {
        return Err("this computer no longer keeps this history".into());
    }
    fastctx_platform::ensure_private_dir(&folder.path).map_err(|error| error.to_string())?;
    if node.replicas.state.folders.lock().expect("never poisoned").get(&folder.path) == Some(&folder) {
        return Ok(());
    }
    state::save_folder(&node.world, &folder).map_err(|error| error.to_string())?;
    let mut folders = node.replicas.state.folders.lock().expect("never poisoned");
    folders.retain(|path, existing| {
        path == &folder.path
            || existing.account.hub != folder.account.hub
            || state::scope_key(&existing.scope) != state::scope_key(&folder.scope)
    });
    folders.insert(folder.path.clone(), folder);
    Ok(())
}

/// `prune_now`, where reading and writing the disk may block.
pub(crate) async fn prune(node: &Arc<Node>) -> Result<(), String> {
    let node = node.clone();
    fastctx_platform::task::blocking(move || prune_now(&node)).await
}

/// An absent current view is unknown, not a revocation. A removed membership is final.
pub(crate) fn prune_now(node: &Node) -> Result<(), String> {
    node.replicas.state.load(&node.world).map_err(|error| error.to_string())?;
    let routes: Vec<_> = node
        .replicas
        .state
        .routes
        .lock()
        .expect("never poisoned")
        .iter()
        .map(|(id, route)| (id.clone(), route.clone()))
        .collect();
    for (id, route) in routes {
        let _publication = node.replicas.state.publication.write().expect("never poisoned");
        if node.link_of(&route.account.hub).is_some_and(|link| !link.view_current()) {
            continue;
        }
        if permitted(node, &route).is_err() {
            state::delete(&node.world, &route).map_err(|error| error.to_string())?;
            node.replicas.state.remove(&id);
        }
    }
    let folders: Vec<_> = node.replicas.state.folders.lock().expect("never poisoned").values().cloned().collect();
    for folder in folders {
        let _publication = node.replicas.state.publication.write().expect("never poisoned");
        if node.link_of(&folder.account.hub).is_some_and(|link| !link.view_current()) {
            continue;
        }
        if !scope_allowed(node, &folder) {
            state::delete_folder(&node.world, &folder).map_err(|error| error.to_string())?;
            node.replicas.state.folders.lock().expect("never poisoned").remove(&folder.path);
        }
    }
    Ok(())
}

/// `catalog_now`, where writing the disk may block.
pub(crate) async fn catalog(
    node: &Arc<Node>,
    hub: fastctx_types::access::HubId,
    catalog: fastctx_types::replicas::Catalog,
    known: Vec<(String, Route)>,
    folders: Vec<Folder>,
) -> Result<(), String> {
    let node = node.clone();
    fastctx_platform::task::blocking(move || catalog_now(&node, &hub, &catalog, known, folders)).await
}

/// Removes records absent from an authoritative catalog, without confusing a concurrent new topic with deletion.
pub(crate) fn catalog_now(
    node: &Node,
    hub: &fastctx_types::access::HubId,
    catalog: &fastctx_types::replicas::Catalog,
    known: Vec<(String, Route)>,
    folders: Vec<Folder>,
) -> Result<(), String> {
    let scopes: std::collections::BTreeSet<_> = catalog.scopes.iter().map(state::scope_key).collect();
    let contains = |route: &Route| match &route.head.document {
        Document::History { scope, .. } => scopes.contains(&state::scope_key(scope)),
        Document::Plan { owner } => scopes.contains(&format!("conversation:{}:{}", route.head.owner, owner.resident)),
    };
    for (id, route) in known {
        if &route.account.hub != hub || contains(&route) {
            continue;
        }
        let _publication = node.replicas.state.publication.write().expect("never poisoned");
        if node
            .replicas
            .state
            .routes
            .lock()
            .expect("never poisoned")
            .get(&id)
            .is_some_and(|current| current.epoch == route.epoch)
        {
            state::delete(&node.world, &route).map_err(|error| error.to_string())?;
            node.replicas.state.remove(&id);
        }
    }
    for folder in folders {
        if &folder.account.hub != hub || scopes.contains(&state::scope_key(&folder.scope)) {
            continue;
        }
        let _publication = node.replicas.state.publication.write().expect("never poisoned");
        state::delete_folder(&node.world, &folder).map_err(|error| error.to_string())?;
        node.replicas.state.folders.lock().expect("never poisoned").remove(&folder.path);
    }
    Ok(())
}
