//! Durable plan copies on their working computer. Native file notifications
//! and hub record invalidations schedule only the affected copies. Each copy
//! has its own gate; a disconnected account never holds another copy's gate.

use crate::{Node, admission::Asker};
use fastctx_types::{
    Address,
    access::{Acting, HubAccount},
    account_records::Changed,
    ipc::Failure,
    placement::{KIND, Location},
    plan_copies::{Reply, Request},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
mod journal;
mod retire;

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

pub(crate) struct Copies {
    slots: Mutex<HashMap<String, Arc<Slot>>>,
    watcher: Mutex<Option<fastctx_watch::Watcher>>,
    /// Copies synchronized at once, so a computer holding many does not ask
    /// the hub for all of them together after a restart or reconnect.
    at_once: tokio::sync::Semaphore,
}

impl Default for Copies {
    fn default() -> Self {
        Self { slots: Mutex::default(), watcher: Mutex::default(), at_once: tokio::sync::Semaphore::new(AT_ONCE) }
    }
}

const AT_ONCE: usize = 8;
struct Slot {
    key: String,
    account: HubAccount,
    owner: Address,
    path: PathBuf,
    state: tokio::sync::Mutex<State>,
    dirty: AtomicBool,
    running: AtomicBool,
    watching: AtomicBool,
}
#[derive(Clone, Serialize, Deserialize)]
struct State {
    account: HubAccount,
    owner: Address,
    opened: jiff::Timestamp,
    placement: u64,
    base: Option<u64>,
    text: String,
    initial: bool,
    #[serde(default)]
    submission: Option<journal::Submission>,
    #[serde(default)]
    publication: Option<journal::Publication>,
    #[serde(default)]
    retiring: bool,
    #[serde(default)]
    retirement_base: Option<u64>,
    #[serde(default)]
    retirement_digest: Option<String>,
}
fn failure(error: impl std::fmt::Display) -> Failure {
    Failure::new(error.to_string())
}
fn directory(node: &Node) -> PathBuf {
    node.world.path().join("working-copies")
}
fn identity(account: &HubAccount, owner: &Address) -> Result<String, Failure> {
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(&(account, owner)).map_err(failure)?)))
}
fn file(node: &Node, owner: &Address) -> PathBuf {
    node.world.owner(&node.names(), owner).path().join("plan.md")
}
fn read(path: &Path) -> Result<Option<String>, Failure> {
    match std::fs::read_to_string(path) {
        Ok(text) => {
            fastctx_types::plan::validate(&text).map_err(failure)?;
            Ok(Some(text))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(failure(error)),
    }
}
fn keep(node: &Node, key: &str, state: &State) -> Result<(), Failure> {
    fastctx_platform::ensure_private_dir(&directory(node)).map_err(failure)?;
    fastctx_platform::publish(
        &directory(node).join(format!("{key}.json")),
        &serde_json::to_vec(state).map_err(failure)?,
        fastctx_platform::Permissions::Private,
    )
    .map_err(failure)
}

fn register(node: &Arc<Node>, state: State) -> Result<Arc<Slot>, Failure> {
    let key = identity(&state.account, &state.owner)?;
    let path = file(node, &state.owner);
    let slot = {
        let mut slots = node.copies.slots.lock().expect("never poisoned");
        if let Some(slot) = slots.get(&key) {
            return Ok(slot.clone());
        }
        if slots.values().any(|slot| slot.path == path) {
            return Err(failure("another account already owns this plan-copy path"));
        }
        fastctx_platform::ensure_private_dir(path.parent().expect("conversation directory")).map_err(failure)?;
        keep(node, &key, &state)?;
        let slot = Arc::new(Slot {
            key: key.clone(),
            account: state.account.clone(),
            owner: state.owner.clone(),
            path,
            state: tokio::sync::Mutex::new(state),
            dirty: AtomicBool::new(false),
            running: AtomicBool::new(false),
            watching: AtomicBool::new(true),
        });
        slots.insert(key, slot.clone());
        slot
    };
    if let Some(watcher) = node.copies.watcher.lock().expect("never poisoned").as_mut() {
        watcher
            .watch(slot.path.parent().expect("conversation directory"), fastctx_watch::RecursiveMode::NonRecursive)
            .map_err(failure)?;
    }
    Ok(slot)
}

pub(crate) async fn activate(
    node: &Arc<Node>,
    asker: &Asker,
    owner: Address,
    placement: u64,
) -> Result<Location, Failure> {
    let account = HubAccount { hub: asker.hub.clone(), account: asker.account.clone() };
    let link = node.link_of(&account.hub).ok_or_else(|| failure("the copy's hub is unavailable"))?;
    let acting = Acting::one(account.hub.clone(), account.account.clone());
    node.authorize_machine(&acting, &node.machine().into())?;
    if !super::owned(crate::authorization::current_view(&link)?.member(&node.machine()), &account.account) {
        return Err(Failure::refused("working copies belong on the account's own computers"));
    }
    let (version, place) = super::location::read_place(&link, &account, &owner).await?;
    if version != Some(placement)
        || !place.is_some_and(|place| place.machine.is_some_and(|machine| node.is_me(&machine)))
    {
        return Err(Failure::not_run("this is no longer the conversation's working computer"));
    }
    let opened = crate::authorization::current_view(&link)?
        .account(&account.account)
        .ok_or_else(|| failure("the copy's account is unavailable"))?
        .opened;
    let slot = register(
        node,
        State {
            account,
            owner,
            opened,
            placement,
            base: None,
            text: String::new(),
            initial: true,
            submission: None,
            publication: None,
            retiring: false,
            retirement_base: None,
            retirement_digest: None,
        },
    )?;
    synchronize(node, &slot).await?;
    request(node, slot.clone());
    Ok(Location { machine: node.machine(), path: crate::world::native(&slot.path) })
}

async fn synchronize(node: &Arc<Node>, slot: &Arc<Slot>) -> Result<(), Failure> {
    let mut state = slot.state.lock().await;
    let link = node.link_of(&state.account.hub).ok_or_else(|| failure("the copy's hub is unavailable"))?;
    let acting = Acting::one(state.account.hub.clone(), state.account.account.clone());
    node.authorize_machine(&acting, &node.machine().into())?;
    if !super::owned(crate::authorization::current_view(&link)?.member(&node.machine()), &state.account.account) {
        return Err(Failure::refused("this computer no longer owns the copy's account"));
    }
    if crate::authorization::current_view(&link)?
        .account(&state.account.account)
        .is_none_or(|account| account.opened != state.opened)
    {
        return Err(Failure::refused("the working copy belongs to an earlier account incarnation"));
    }
    let (placement, place) = super::location::read_place(&link, &state.account, &state.owner).await?;
    let Some(placement) = placement else { return Err(failure("the working-copy record is unavailable")) };
    let retained = place.as_ref().and_then(|place| place.machine.as_ref()).is_some_and(|machine| node.is_me(machine));
    slot.watching.store(true, Ordering::Release);
    let mut page = link.plan_read(&state.account.account, state.owner.resident.clone()).await.map_err(failure)?;
    if link.plan_of_another(&page, &state.owner).is_some() {
        return Err(failure("the plan record belongs to a different conversation"));
    }
    if page.deleted() {
        discard(node, slot)?;
        return Ok(());
    }
    if state.retiring {
        retire::finish(node, slot, &mut state, &link).await?;
        if read(&slot.path)?.is_some() || read(&retire::staged(slot))?.is_some() {
            slot.dirty.store(true, Ordering::Release);
            return Ok(());
        }
        if !retained {
            link.follow_record(&state.account.account, fastctx_types::plan::KIND, state.owner.resident.as_str(), false)
                .await
                .map_err(failure)?;
            slot.watching.store(false, Ordering::Release);
            return Ok(());
        }
        state.retiring = false;
        state.initial = true;
        keep(node, &slot.key, &state)?;
    }
    if state.submission.is_some() || state.publication.is_some() {
        journal::settle(node, slot, &mut state, &link).await?;
        page = link.plan_read(&state.account.account, state.owner.resident.clone()).await.map_err(failure)?;
    }
    let mut on_disk = read(&slot.path)?;
    if !state.initial && on_disk.as_deref() == Some("") && !state.text.is_empty() {
        // An editor may empty the file before writing it again; only one
        // that stays empty is the plan emptied.
        tokio::time::sleep(EMPTIED).await;
        on_disk = read(&slot.path)?;
        if on_disk.as_deref() != Some("") {
            slot.dirty.store(true, Ordering::Release);
            return Ok(());
        }
    }
    let dirty = match &on_disk {
        // A missing file is not an edit that empties the plan: editors save
        // by replacing the file, and the hub's text is written back below.
        None => false,
        Some(text) if state.initial => !had(&link, &state, &page, text).await?,
        Some(text) => *text != state.text,
    };
    if dirty {
        let text = on_disk.clone().unwrap_or_default();
        let id = fastctx_net::plan_merge::submission(&state.owner, state.base, &text);
        state.submission = Some(journal::Submission { id, base: state.base, text });
        keep(node, &slot.key, &state)?;
        journal::settle(node, slot, &mut state, &link).await?;
        page = link.plan_read(&state.account.account, state.owner.resident.clone()).await.map_err(failure)?;
    }
    let on_disk = read(&slot.path)?;
    if !state.initial && on_disk.as_ref().is_some_and(|text| *text != state.text) {
        slot.dirty.store(true, Ordering::Release);
        return Ok(());
    }
    if page.version() != state.base || state.initial || on_disk.is_none() {
        let text = page.body.as_ref().map_or_else(String::new, |body| body.markdown.clone());
        state.publication = Some(journal::Publication { before: on_disk, text, page });
        keep(node, &slot.key, &state)?;
        journal::settle(node, slot, &mut state, &link).await?;
    }
    state.placement = placement;
    keep(node, &slot.key, &state)?;
    if !retained {
        if read(&slot.path)?.as_deref().unwrap_or_default() != state.text {
            slot.dirty.store(true, Ordering::Release);
            return Ok(());
        }
        state.retiring = true;
        state.retirement_base = state.base;
        state.retirement_digest = Some(format!("{:x}", Sha256::digest(state.text.as_bytes())));
        keep(node, &slot.key, &state)?;
        retire::finish(node, slot, &mut state, &link).await?;
    }
    Ok(())
}

/// How long a file emptied while it held a plan is given to be written again.
const EMPTIED: std::time::Duration = std::time::Duration::from_secs(1);

/// Whether the hub has had `text` as this plan, now or in an earlier
/// version. A file found when a copy is first activated that the hub has had
/// is an old copy, which the hub's plan replaces, not an edit made against
/// nothing.
async fn had(
    link: &fastctx_net::link::Link,
    state: &State,
    page: &fastctx_types::plan::Page,
    text: &str,
) -> Result<bool, Failure> {
    if page.body.as_ref().is_some_and(|body| body.markdown == text) {
        return Ok(true);
    }
    if page.version().is_none() {
        return Ok(false);
    }
    match link
        .plan_copy(&state.account.account, Request::Contains { owner: state.owner.clone(), markdown: text.into() })
        .await
        .map_err(failure)?
    {
        Reply::Contains { found } => Ok(found),
        _ => Err(failure("the hub did not say whether it had the plan")),
    }
}

fn discard(node: &Node, slot: &Slot) -> Result<(), Failure> {
    for path in [&slot.path, &retire::staged(slot), &directory(node).join(format!("{}.json", slot.key))] {
        match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(failure(error)),
        }
    }
    node.copies.slots.lock().expect("never poisoned").remove(&slot.key);
    Ok(())
}

fn request(node: &Arc<Node>, slot: Arc<Slot>) {
    slot.dirty.store(true, Ordering::Release);
    if slot.running.swap(true, Ordering::AcqRel) {
        return;
    }
    let weak = Arc::downgrade(node);
    tokio::spawn(async move {
        let mut retry = crate::again::Again::default();
        let mut previous = String::new();
        loop {
            slot.dirty.store(false, Ordering::Release);
            let Some(node) = weak.upgrade() else { return };
            if !node.copies.slots.lock().expect("never poisoned").contains_key(&slot.key) {
                break;
            }
            let synchronized = {
                let _turn = node.copies.at_once.acquire().await.expect("never closed");
                synchronize(&node, &slot).await
            };
            match synchronized {
                Ok(()) => {
                    previous.clear();
                    retry = Default::default();
                }
                Err(error) => {
                    if previous != error.message {
                        node.log.line(format_args!("synchronizing plan copy {}: {}", slot.key, error.message));
                        previous = error.message;
                    }
                    drop(node);
                    tokio::time::sleep(retry.next()).await;
                    continue;
                }
            }
            slot.running.store(false, Ordering::Release);
            if !slot.dirty.load(Ordering::Acquire) || slot.running.swap(true, Ordering::AcqRel) {
                return;
            }
        }
        slot.running.store(false, Ordering::Release);
    });
}

/// Called by the link's authenticated, metadata-only account invalidation.
pub(crate) fn changed(node: &Arc<Node>, hub: &fastctx_types::access::HubId, change: &Changed) {
    if change.metadata.kind != fastctx_types::plan::KIND && change.metadata.kind != KIND {
        return;
    }
    let slots: Vec<_> = node.copies.slots.lock().expect("never poisoned").values().cloned().collect();
    for slot in slots {
        if slot.account.hub == *hub
            && slot.account.account == change.account
            && slot.owner.resident.as_str() == change.metadata.key
            && (change.metadata.kind != fastctx_types::plan::KIND || slot.watching.load(Ordering::Acquire))
        {
            request(node, slot);
        }
    }
}

pub(crate) fn start(node: &Arc<Node>) {
    let weak = Arc::downgrade(node);
    let runtime = tokio::runtime::Handle::current();
    let mut watcher = fastctx_watch::Watcher::new(move |event: fastctx_watch::Result<fastctx_watch::Event>| {
        let _entered = runtime.enter();
        let Some(node) = weak.upgrade() else { return };
        match event {
            Ok(event) => {
                if !event
                    .paths
                    .iter()
                    .any(|path| path.file_name().is_some_and(|name| name == "plan.md" || name == ".plan-retiring.md"))
                {
                    return;
                }
                let slots: Vec<_> = node
                    .copies
                    .slots
                    .lock()
                    .expect("never poisoned")
                    .values()
                    .filter(|slot| event.paths.iter().any(|path| *path == slot.path || *path == retire::staged(slot)))
                    .cloned()
                    .collect();
                for slot in slots {
                    request(&node, slot);
                }
            }
            Err(error) => node.log.line(format_args!("watching plan copies: {error}")),
        }
    })
    .following_links(false);
    if let Err(error) = watcher.watch_within(&node.world.conversations(), |path| path.components().count() <= 1) {
        node.log.line(format_args!("watching working-copy directories: {error}"));
    }
    *node.copies.watcher.lock().expect("never poisoned") = Some(watcher);
    let restored = (|| -> Result<(), Failure> {
        let entries = match std::fs::read_dir(directory(node)) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(failure(error)),
        };
        for entry in entries {
            let path = entry.map_err(failure)?.path();
            if path.extension().is_none_or(|extension| extension != "json") {
                continue;
            }
            // One copy that cannot be restored leaves the others theirs.
            let restored = std::fs::read(&path)
                .map_err(failure)
                .and_then(|bytes| serde_json::from_slice::<State>(&bytes).map_err(failure))
                .and_then(|state| register(node, state));
            match restored {
                Ok(slot) => request(node, slot),
                Err(error) => node.log.line(format_args!("restoring plan copy {}: {}", path.display(), error.message)),
            }
        }
        Ok(())
    })();
    if let Err(error) = restored {
        node.log.line(format_args!("restoring plan copies: {}", error.message));
    }
}

/// Reconnection recovers invalidations missed while this computer was away.
pub(crate) fn refresh(node: &Arc<Node>, hub: &fastctx_types::access::HubId) {
    let slots: Vec<_> = node
        .copies
        .slots
        .lock()
        .expect("never poisoned")
        .values()
        .filter(|slot| slot.account.hub == *hub)
        .cloned()
        .collect();
    for slot in slots {
        request(node, slot);
    }
}
