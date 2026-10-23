//! Account watches dispatch bounded work; sleeping sentinels have no resident task.
use crate::Node;
use fastctx_sentinel::{lifecycle, ports::Fired, resident::Host};
use fastctx_types::{
    SentinelId,
    access::{Acting, HubAccount},
};
use std::{
    collections::{BTreeSet, HashMap, HashSet},
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};
use tokio::sync::Semaphore;

pub(super) struct Worker {
    pub engine: OnceLock<Arc<Host>>,
    slots: Arc<Semaphore>,
    active: Mutex<HashSet<(HubAccount, SentinelId)>>,
    errors: Mutex<HashMap<String, String>>,
}
impl Default for Worker {
    fn default() -> Self {
        Self {
            engine: OnceLock::new(),
            slots: Arc::new(Semaphore::new(8)),
            active: Mutex::new(HashSet::new()),
            errors: Mutex::new(HashMap::new()),
        }
    }
}
pub(super) fn engine(node: &Arc<Node>) -> Result<Arc<Host>, String> {
    if let Some(host) = node.sentinels.worker.engine.get() {
        return Ok(host.clone());
    }
    let records = node.sentinels.records.get().ok_or("sentinel account record service is not connected")?.clone();
    let timers = node.sentinels.timers.get().ok_or("sentinel durable timer service is not connected")?.clone();
    Ok(node
        .sentinels
        .worker
        .engine
        .get_or_init(|| {
            Arc::new(Host::new(
                node.world.path().join("sentinels"),
                records,
                timers,
                Arc::new(super::effects::Effects(Arc::downgrade(node))),
            ))
        })
        .clone())
}
pub(crate) fn start(node: &Arc<Node>) {
    let node = node.clone();
    tokio::spawn(async move {
        let mut watchers = HashMap::new();
        let mut timers_started = false;
        let mut ways = node.ways.subscribe();
        loop {
            if let Ok(host) = engine(&node) {
                if !timers_started {
                    start_timers(&node);
                    timers_started = true;
                }
                let mut scopes = BTreeSet::new();
                for link in node.links() {
                    for account in node.accounts_on(&link, &Acting::Here) {
                        scopes.insert(HubAccount { hub: link.membership().id.clone(), account });
                    }
                }
                match host.known() {
                    Ok(known) => {
                        for registration in known {
                            scopes.insert(registration.scope);
                        }
                    }
                    Err(error) => report(&node, "discovery", Some(error)),
                }
                for scope in scopes {
                    if watchers.get(&scope).is_none_or(|task: &tokio::task::JoinHandle<()>| task.is_finished()) {
                        let (node, scope_copy) = (node.clone(), scope.clone());
                        watchers.insert(scope, tokio::spawn(async move { watch_scope(node, scope_copy).await }));
                    }
                }
            }
            tokio::select! { _=ways.changed()=>{}, ()=tokio::time::sleep(Duration::from_secs(10))=>{} }
        }
    });
}
async fn watch_scope(node: Arc<Node>, scope: HubAccount) {
    let key = format!("account {:?}", scope);
    let mut retry = crate::again::Again::default();
    loop {
        let result: Result<(), String> = async {
            let host = engine(&node)?;
            let records = &host.records;
            let mut changes = tokio::time::timeout(Duration::from_secs(60), records.watch(&scope, lifecycle::KIND))
                .await
                .map_err(|_| "subscribing to sentinel records timed out")?
                .map_err(|e| e.to_string())?;
            let mut settings =
                tokio::time::timeout(Duration::from_secs(60), records.watch(&scope, fastctx_sentinel::settings::KIND))
                    .await
                    .map_err(|_| "subscribing to sentinel settings timed out")?
                    .map_err(|e| e.to_string())?;
            let mut ways = node.ways.subscribe();
            loop {
                let heads = tokio::time::timeout(Duration::from_secs(60), records.list(&scope, lifecycle::KIND))
                    .await
                    .map_err(|_| "listing sentinel assignments timed out")?
                    .map_err(|e| e.to_string())?;
                let mut ids = BTreeSet::new();
                for head in heads {
                    ids.insert(head.key.parse::<SentinelId>().map_err(|e| e.to_string())?);
                }
                for registration in host.known()? {
                    if registration.scope == scope {
                        ids.insert(registration.definition.id);
                    }
                }
                for id in ids {
                    enqueue(&node, scope.clone(), id, None).await;
                }
                report(&node, &key, None);
                super::keys::forget_if_moved(&node, &scope).await;
                tokio::select! {
                    changed=changes.recv()=>{changed.map_err(|e|e.to_string())?;},
                    changed=settings.recv()=>{changed.map_err(|e|e.to_string())?;},
                    changed=ways.changed()=>{changed.map_err(|e|e.to_string())?;},
                    ()=tokio::time::sleep(Duration::from_secs(60))=>{},
                }
            }
        }
        .await;
        if let Err(error) = result {
            report(&node, &key, Some(error));
        }
        tokio::time::sleep(retry.next()).await;
    }
}
fn start_timers(node: &Arc<Node>) {
    let node = node.clone();
    tokio::spawn(async move {
        let timers = node.sentinels.timers.get().expect("engine has timer service");
        let mut retry = crate::again::Again::default();
        loop {
            let subscribed = tokio::time::timeout(Duration::from_secs(60), timers.subscribe()).await;
            let result = match subscribed {
                Ok(Ok(mut events)) => {
                    retry.reset();
                    loop {
                        match events.recv().await {
                            Ok(fired) => {
                                use fastctx_sentinel::ports::TimerAction;
                                let (scope, id) = match &fired.timer.action {
                                    TimerAction::RunReflex { scope, id, .. }
                                    | TimerAction::WakeSentinel { scope, id, .. } => (scope.clone(), id.clone()),
                                };
                                enqueue(&node, scope, id, Some(fired)).await;
                            }
                            Err(error) => break error.to_string(),
                        }
                    }
                }
                Ok(Err(error)) => error,
                Err(_) => "subscribing to sentinel timers timed out".into(),
            };
            report(&node, "timers", Some(result));
            tokio::time::sleep(retry.next()).await;
        }
    });
}
async fn enqueue(node: &Arc<Node>, scope: HubAccount, id: SentinelId, fired: Option<Fired>) {
    let Ok(slot) = node.sentinels.worker.slots.clone().acquire_owned().await else {
        return;
    };
    let key = (scope.clone(), id.clone());
    if !node.sentinels.worker.active.lock().expect("never poisoned").insert(key.clone()) {
        return;
    }
    let node = node.clone();
    tokio::spawn(async move {
        let _slot = slot;
        let result = async {
            let host = engine(&node)?;
            delete_if_owner_deleted(&node, &host, &scope, &id).await?;
            if let Some(fired) = fired {
                host.fire(&fired).await?;
                tokio::time::timeout(
                    Duration::from_secs(60),
                    node.sentinels.timers.get().expect("engine has timers").acknowledge(&fired),
                )
                .await
                .map_err(|_| "acknowledging sentinel timer timed out")??;
            } else {
                host.reconcile(&scope, &id).await?;
            }
            super::delivery::pending(&node, &host, &scope, &id).await?;
            Ok::<(), String>(())
        }
        .await;
        report(&node, &format!("{} {:?}", id, scope), result.err());
        node.sentinels.worker.active.lock().expect("never poisoned").remove(&key);
    });
}
async fn delete_if_owner_deleted(node: &Node, host: &Host, scope: &HubAccount, id: &SentinelId) -> Result<(), String> {
    let Some((version, registration)) =
        lifecycle::get(host.records.as_ref(), scope, id).await.map_err(|e| e.to_string())?
    else {
        return Ok(());
    };
    let owner = &registration.definition.owner;
    if !node.is_me(&owner.machine) {
        return Ok(());
    }
    let deleted: Option<Vec<fastctx_types::Address>> = fastctx_agent::state::load(
        &node.world.path().join("sentinel-deleted-owners").join(format!("{}.json", owner.resident)),
    )?;
    if deleted.is_some_and(|owners| owners.contains(owner)) {
        tokio::time::timeout(
            Duration::from_secs(60),
            host.records.compare_exchange(scope, lifecycle::KIND, id.as_str(), Some(version), None),
        )
        .await
        .map_err(|_| "deleting an orphan sentinel timed out")?
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}
pub(crate) fn report(node: &Node, key: &str, error: Option<String>) {
    let changed = {
        let mut errors = node.sentinels.worker.errors.lock().expect("never poisoned");
        match error {
            Some(error) if errors.get(key) != Some(&error) => {
                errors.insert(key.into(), error.clone());
                Some(error)
            }
            None => { errors.remove(key); None }
            _ => None,
        }
    };
    if let Some(error) = changed { node.log.line(format_args!("sentinel {key}: {error}")); }
}
