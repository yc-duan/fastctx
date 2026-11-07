//! Short local publication transactions never wait for a document's network operation.

use super::{
    bodies::Text,
    state::{self, Copy, Folder, Projected, Route},
};
use crate::Node;
use fastctx_types::replicas::{Document, Scope};
use std::sync::Arc;

/// Whether the hub ended this computer's place in its network: what it kept
/// for that network goes, whatever the last view said.
fn left(link: &fastctx_net::link::Link) -> bool {
    matches!(link.status(), fastctx_net::link::Status::Stopped { .. })
        || fastctx_net::link::state::read_or::<Option<fastctx_net::link::state::Ended>>(
            &fastctx_net::link::NetDir::new(link.dir().to_path_buf()).ended(),
            None,
        )
        .is_ok_and(|ended| ended.is_some())
}

/// Whether what this computer knows of a hub's view is unknown for now: a
/// line that is down is no revocation; a place the hub ended is.
fn unknown(node: &Node, hub: &fastctx_types::access::HubId) -> bool {
    node.link_of(hub).is_some_and(|link| !link.view_current() && !left(&link))
}

fn scope_allowed(node: &Node, folder: &Folder) -> bool {
    let Some(link) = node.link_of(&folder.account.hub) else { return false };
    if left(&link) {
        return false;
    }
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
    if left(&link) {
        return Err("this computer left the replica's network".into());
    }
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

/// `save_now`, where writing the disk may block. `previous` is the text the
/// copy showed before this change, for a copy that never kept what it
/// projected.
pub(crate) async fn save(
    node: &Arc<Node>,
    copy: &mut Copy,
    publish: bool,
    previous: Option<&Text>,
) -> Result<(), String> {
    let (node, saving, previous) = (node.clone(), copy.clone(), previous.cloned());
    copy.projected =
        fastctx_platform::task::blocking(move || save_now(&node, &saving, publish, previous.as_ref())).await?;
    Ok(())
}

/// Checks again inside the same local transaction used by revocation. What
/// the copy's path holds afterwards.
pub(crate) fn save_now(
    node: &Node,
    copy: &Copy,
    publish: bool,
    previous: Option<&Text>,
) -> Result<Option<Projected>, String> {
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
    let mut kept = copy.clone();
    // An edit already accepted or queued stays durable when history is paused.
    if publish && super::sync::enabled(node, &copy.account, &copy.head.document) {
        kept.projected = project(node, copy, previous)?;
    }
    if let Some(warning) = state::save(&node.world, &kept).map_err(|error| error.to_string())? {
        node.log.line(warning);
    }
    node.replicas.state.relocated(copy);
    node.replicas.state.routes.lock().expect("never poisoned").insert(copy.id(), route);
    drop(_publication);
    if changed {
        crate::context::invalidations::document(node, &copy.account.hub, &copy.head.document);
    }
    Ok(kept.projected)
}

/// Brings the copy's path to what the copy shows. What it holds afterwards.
fn project(node: &Node, copy: &Copy, previous: Option<&Text>) -> Result<Option<Projected>, String> {
    if matches!(copy.head.document, Document::History { .. }) && copy.head.version.is_none() && copy.pending.is_empty()
    {
        return Ok(copy.projected.clone());
    }
    if copy.head.retired && copy.pending.is_empty() {
        super::files::retire(&copy.path, copy.projected.as_ref(), previous).map_err(|error| error.to_string())?;
        return Ok(None);
    }
    let text = super::bodies::read(&node.world, &copy.id(), copy.effective()).map_err(|error| error.to_string())?;
    super::files::publish(&copy.path, &text, copy.projected.as_ref(), previous)
        .map(Some)
        .map_err(|error| error.to_string())
}

/// `repair_now`, where reading and writing the disk may block.
pub(crate) async fn repair(node: &Arc<Node>, copy: &mut Copy) -> Result<(), String> {
    let (node, repairing) = (node.clone(), copy.clone());
    if let Some(projected) = fastctx_platform::task::blocking(move || repair_now(&node, &repairing)).await? {
        copy.projected = projected;
    }
    Ok(())
}

/// Restores the copy's path when it no longer holds what the copy shows,
/// telling it only by the file's length and time while those have not
/// moved. What the path holds afterwards, when that changed and was kept.
pub(crate) fn repair_now(node: &Node, copy: &Copy) -> Result<Option<Option<Projected>>, String> {
    if !super::sync::enabled(node, &copy.account, &copy.head.document) {
        return Ok(None);
    }
    if node.link_of(&copy.account.hub).is_some_and(|link| !link.view().shared_records) {
        return Ok(None);
    }
    let effective = copy.effective();
    let shown = if copy.head.retired && copy.pending.is_empty() {
        // A retired copy shows nothing.
        fastctx_platform::disk::symlink_metadata(&copy.path).is_err()
    } else {
        copy.projected
            .as_ref()
            .is_some_and(|projected| projected.text == *effective && super::files::intact(&copy.path, projected))
    };
    if shown {
        return Ok(None);
    }
    let _publication = node.replicas.state.publication.read().expect("never poisoned");
    // One no longer kept here is withdrawn, not restored.
    if permitted(node, &Route::from(copy)).is_err() {
        return Ok(None);
    }
    let projected = project(node, copy, Some(effective))?;
    if projected == copy.projected {
        return Ok(None);
    }
    let mut kept = copy.clone();
    kept.projected = projected.clone();
    if let Some(warning) = state::save(&node.world, &kept).map_err(|error| error.to_string())? {
        node.log.line(warning);
    }
    Ok(Some(projected))
}

/// Takes the projections away from the paths the copy moved from, then
/// forgets them. A move withdraws nothing: what was changed in one out of
/// band is kept aside there, as when a document is retired. `shown` is what
/// the copy last projected and `previous` the text it showed.
pub(crate) async fn leave(
    node: &Arc<Node>,
    copy: &mut Copy,
    shown: Option<&Projected>,
    previous: &Text,
) -> Result<(), String> {
    if copy.left.is_empty() || !super::sync::enabled(node, &copy.account, &copy.head.document) {
        return Ok(());
    }
    let (left, shown, previous) = (copy.left.clone(), shown.cloned(), previous.clone());
    fastctx_platform::task::blocking(move || {
        for path in &left {
            super::files::retire(path, shown.as_ref(), Some(&previous))?;
        }
        Ok::<_, std::io::Error>(())
    })
    .await
    .map_err(|error| error.to_string())?;
    let mut kept = copy.clone();
    kept.left.clear();
    save(node, &mut kept, false, None).await?;
    *copy = kept;
    Ok(())
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
    // The folder also names the conversation whose plan this computer keeps;
    // its directory is made only where history is offered.
    if crate::history::switch::enabled(node, &folder.account) {
        fastctx_platform::ensure_private_dir(&folder.path).map_err(|error| error.to_string())?;
    }
    if node.replicas.state.folders.lock().expect("never poisoned").get(&folder.path) == Some(&folder) {
        return Ok(());
    }
    if let Some(warning) = state::save_folder(&node.world, &folder).map_err(|error| error.to_string())? {
        node.log.line(warning);
    }
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
    if !super::cleanup::started(node) {
        super::cleanup::schedule(node);
        return Ok(());
    }
    let working = node.clone();
    let result = fastctx_platform::task::blocking(move || prune_now(&working)).await;
    if result.is_err() {
        super::cleanup::schedule(node);
    }
    result
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
    let mut errors = Vec::new();
    for (id, route) in routes {
        let _publication = node.replicas.state.publication.write().expect("never poisoned");
        let current = node.replicas.state.routes.lock().expect("never poisoned").get(&id).cloned();
        let Some(current) = current else { continue };
        if current != route {
            errors.push("a replica changed during withdrawal; its current route will be checked again".into());
            continue;
        }
        if unknown(node, &route.account.hub) {
            continue;
        }
        if permitted(node, &route).is_err() {
            match state::delete(&node.world, &route) {
                Ok(()) => node.replicas.state.remove(&id),
                Err(error) => errors.push(error.to_string()),
            }
        }
    }
    let folders: Vec<_> = node.replicas.state.folders.lock().expect("never poisoned").values().cloned().collect();
    for folder in folders {
        let _publication = node.replicas.state.publication.write().expect("never poisoned");
        if unknown(node, &folder.account.hub) || !current(node, &folder) {
            continue;
        }
        if !scope_allowed(node, &folder)
            && let Err(error) = delete_folder(node, &folder)
        {
            errors.push(error.to_string());
        }
    }
    match errors.first() {
        Some(first) => Err(format!("{} replica withdrawals failed; the first: {first}", errors.len())),
        None => Ok(()),
    }
}

fn delete_folder(node: &Node, folder: &Folder) -> std::io::Result<()> {
    let scope = state::scope_key(&folder.scope);
    let has_copies = node.replicas.state.routes.lock().expect("never poisoned").values().any(|route| {
        route.account.hub == folder.account.hub
            && match &route.head.document {
                Document::History { scope: kept, .. } => state::scope_key(kept) == scope,
                Document::Plan { owner } => format!("conversation:{}:{}", route.head.owner, owner.resident) == scope,
            }
    });
    if has_copies {
        return Err(std::io::Error::new(std::io::ErrorKind::WouldBlock, "the folder still has copies to withdraw"));
    }
    super::removal::delete(&node.world, &folder.path)?;
    release(node, folder)?;
    state::delete_folder(&node.world, folder)?;
    node.replicas.state.folders.lock().expect("never poisoned").remove(&folder.path);
    Ok(())
}

/// Whether `folder` is still what is kept at its path.
fn current(node: &Node, folder: &Folder) -> bool {
    node.replicas.state.folders.lock().expect("never poisoned").get(&folder.path) == Some(folder)
}

/// Lets go of the directory a conversation of another computer had here
/// only for its copies, binding and all, once nothing else is in it.
fn release(node: &Node, folder: &Folder) -> std::io::Result<()> {
    let Scope::Conversation { conversation } = &folder.scope else { return Ok(()) };
    if node.residents.knows(&conversation.address.resident) {
        return Ok(());
    }
    let Some(directory) = folder.path.parent() else { return Ok(()) };
    let binding = directory.join(".working-copy-owner.json");
    if super::removal::redirected(&node.world, &binding)? {
        return Ok(());
    }
    let entries = match fastctx_platform::disk::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let path = entry?.path();
        if path != binding && path != directory.join("tampering") {
            return Ok(());
        }
    }
    super::removal::delete(&node.world, directory)
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
        if node.replicas.state.routes.lock().expect("never poisoned").get(&id).is_some_and(|current| current == &route)
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
        // The path may since hold another hub's history: only the folder as
        // it was seen is withdrawn.
        if !current(node, &folder) {
            continue;
        }
        delete_folder(node, &folder).map_err(|error| error.to_string())?;
    }
    Ok(())
}
