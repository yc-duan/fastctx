//! Home-side orchestration. Sleeping sentinels occupy only durable records and timers.
mod goal;
mod schedule;
mod storage;

use crate::{
    lifecycle,
    ports::*,
    records::Records,
    state::{State, Store},
};
use fastctx_agent::{FutureResult, Limits, model::Model, state::save};
use fastctx_types::{MachineName, SentinelId, access::HubAccount, sentinel::*, sentinel_control::*};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, Weak},
};
use storage::Ledger;
pub use storage::{Event, Notification};
use tokio::sync::{Mutex as Gate, Semaphore};

pub struct Capabilities {
    pub look: Box<dyn Look>,
    pub web: Box<dyn Web>,
    /// Where it may look this wake; none for a sentinel of the public web.
    pub machines: Vec<crate::ports::Machine>,
}
/// The node checks current account authority and assignment before each effect.
/// Model clients are constructed on the home, never transported to another node.
pub trait Runtime: Send + Sync {
    fn now(&self) -> i64;
    fn assigned<'a>(&'a self, registration: &'a Registration) -> FutureResult<'a, bool>;
    fn online<'a>(&'a self, registration: &'a Registration, machine: &'a MachineName) -> FutureResult<'a, bool>;
    fn capabilities<'a>(
        &'a self,
        registration: &'a Registration,
        directory: &'a Path,
    ) -> FutureResult<'a, Capabilities>;
    fn model<'a>(&'a self, registration: &'a Registration) -> FutureResult<'a, Box<dyn Model>>;
}
type Key = (HubAccount, SentinelId);
/// One gate per active operation, never one sleeping task per sentinel.
pub struct Host {
    root: PathBuf,
    pub records: Arc<dyn Records>,
    timers: Arc<dyn Timers>,
    runtime: Arc<dyn Runtime>,
    gates: Mutex<HashMap<Key, Weak<Gate<()>>>>,
    running: Semaphore,
}
impl Host {
    pub fn new(root: PathBuf, records: Arc<dyn Records>, timers: Arc<dyn Timers>, runtime: Arc<dyn Runtime>) -> Self {
        Self { root, records, timers, runtime, gates: Mutex::new(HashMap::new()), running: Semaphore::new(4) }
    }
    fn gate(&self, scope: &HubAccount, id: &SentinelId) -> Arc<Gate<()>> {
        let mut gates = self.gates.lock().expect("never poisoned");
        gates.retain(|_, gate| gate.strong_count() > 0);
        let slot = gates.entry((scope.clone(), id.clone())).or_default();
        if let Some(gate) = slot.upgrade() {
            return gate;
        }
        let gate = Arc::new(Gate::new(()));
        *slot = Arc::downgrade(&gate);
        gate
    }
    pub async fn inspect(&self, scope: &HubAccount, id: &SentinelId) -> Result<Summary, String> {
        let (_, registration) = lifecycle::get(self.records.as_ref(), scope, id)
            .await
            .map_err(|e| e.to_string())?
            .ok_or("sentinel not found")?;
        if !self.runtime.assigned(&registration).await? { return Err("sentinel is assigned to another home".into()); }
        let store = self.store(&registration);
        let state = store.load()?.ok_or("the sentinel has not begun on this home")?;
        if state.definition != registration.definition {
            return Err("sentinel assignment changed".into());
        }
        Ok(storage::summary(&state))
    }
    /// Reconciles the authoritative definition, interrupted wake, alarms and timers.
    pub async fn reconcile(&self, scope: &HubAccount, id: &SentinelId) -> Result<(), String> {
        let gate = self.gate(scope, id);
        let _guard = gate.lock().await;
        self.reconcile_locked(scope, id, None).await.map(|_| ())
    }
    async fn reconcile_locked(
        &self,
        scope: &HubAccount,
        id: &SentinelId,
        fired: Option<&Fired>,
    ) -> Result<Option<Summary>, String> {
        let current = lifecycle::get(self.records.as_ref(), scope, id).await.map_err(|e| e.to_string())?;
        let Some((version, registration)) = current else {
            self.withdraw_local(scope, id).await?;
            return Ok(None);
        };
        if !self.runtime.assigned(&registration).await? {
            self.withdraw_local(scope, id).await?;
            return Ok(None);
        }
        if let Some(previous) = fastctx_agent::state::load::<Registration>(&self.assignment(scope, id))?
            && previous.definition.generation != registration.definition.generation
        {
            self.withdraw_local(scope, id).await?;
        }
        let store = self.store(&registration);
        let mut state = store.load()?.unwrap_or_else(|| State::new(registration.definition.clone()));
        if state.definition != registration.definition {
            return Err("a sentinel definition changed without a new generation".into());
        }
        let mut ledger = Ledger::load(&store)?;
        save(&self.assignment(scope, id), &registration)?;
        if state.sequence == 0 {
            store.checkpoint(&state)?;
        }
        if let Some(fired) = fired
            && !self.accepts(&registration, &state, &ledger, fired)
        {
            return Ok(Some(storage::summary(&state)));
        }
        let now = self.runtime.now();
        let alarm_until = if state.finished { state.completed_at } else { now };
        ledger.alarms(&registration, &state, alarm_until);
        ledger.save(&store)?;
        if registration.active && !state.finished && state.definition.kind != Kind::Goal {
            let reason = if let Some(pending) = &state.pending {
                Some((pending.id.clone(), pending.reason.clone()))
            } else if state.sequence == 0 && !state.definition.condition.trim().is_empty() {
                Some(("initial".into(), WakeReason::Initial))
            } else {
                self.check_reflex(&store, &mut state, &mut ledger, fired).await?
            };
            if let Some((id, reason)) = reason {
                let _permit = tokio::time::timeout(std::time::Duration::from_secs(30), self.running.acquire())
                    .await
                    .map_err(|_| "the sentinel home is busy")?
                    .map_err(|e| e.to_string())?;
                let pending = crate::state::WakeIntent { id: id.clone(), reason: reason.clone(), at: now };
                state.pending = Some(pending);
                store.checkpoint(&state)?;
                let prepared = tokio::time::timeout(std::time::Duration::from_secs(60), async {
                    let capabilities = self.runtime.capabilities(&registration, store.directory()).await?;
                    let model = self.runtime.model(&registration).await?;
                    Ok::<_, String>((capabilities, model))
                })
                .await
                .map_err(|_| "preparing the sentinel's capabilities timed out".to_string())
                .and_then(|r| r);
                match prepared {
                    Ok((capabilities, model)) => {
                        state = crate::wake::wake(
                            &store,
                            state,
                            &id,
                            reason,
                            self.runtime.now(),
                            &crate::wake::Environment {
                                model: model.as_ref(),
                                look: capabilities.look.as_ref(),
                                web: capabilities.web.as_ref(),
                                machines: &capabilities.machines,
                                limits: &Limits::default(),
                            },
                        )
                        .await?;
                        ledger.offline.clear();
                    }
                    Err(error) => {
                        state.sequence += 1;
                        state.wake_id = id;
                        state.pending = None;
                        state.completed_at = self.runtime.now();
                        state.conclusion = Conclusion::NotJudged { reason: error };
                        state.reflexes.retain(|r| r.id != "retry");
                        state.reflexes.push(crate::wake::retry_reflex(self.runtime.now()));
                        store.commit(&state)?;
                    }
                }
            }
            ledger.alarms(&registration, &state, if state.finished { state.completed_at } else { self.runtime.now() });
            if state.definition.condition.trim().is_empty() {
                state.plan = crate::words::ALARM_PLAN.into();
                if ledger.alarms.len() == state.definition.at.len() {
                    state.finished = true;
                    state.completed_at = self.runtime.now();
                }
                store.checkpoint(&state)?;
            }
        }
        ledger.conclusion(&registration, &state);
        ledger.save(&store)?;
        self.synchronize(&registration, &state, &ledger).await?;
        if state.finished && registration.active {
            let mut finished = registration;
            finished.active = false;
            tokio::time::timeout(
                std::time::Duration::from_secs(60),
                self.records.compare_exchange(
                    scope,
                    lifecycle::KIND,
                    id.as_str(),
                    Some(version),
                    Some(serde_json::to_value(finished).map_err(|e| e.to_string())?),
                ),
            )
            .await
            .map_err(|_| "recording sentinel completion timed out; outcome unknown")?
            .map_err(|e| e.to_string())?;
        }
        Ok(Some(storage::summary(&state)))
    }
    pub async fn fire(&self, fired: &Fired) -> Result<(), String> {
        let (scope, id) = match &fired.timer.action {
            TimerAction::WakeSentinel { scope, id, .. } | TimerAction::RunReflex { scope, id, .. } => (scope, id),
        };
        let gate = self.gate(scope, id);
        let _guard = gate.lock().await;
        self.reconcile_locked(scope, id, Some(fired)).await.map(|_| ())
    }
    pub async fn notifications(&self, scope: &HubAccount, id: &SentinelId) -> Result<Vec<Notification>, String> {
        let gate = self.gate(scope, id);
        let _guard = gate.lock().await;
        let Some((_, registration)) =
            lifecycle::get(self.records.as_ref(), scope, id).await.map_err(|e| e.to_string())?
        else {
            return Ok(Vec::new());
        };
        if !self.runtime.assigned(&registration).await? {
            return Ok(Vec::new());
        }
        let ledger = Ledger::load(&self.store(&registration))?;
        Ok(ledger.notifications.into_iter().filter(|n| !ledger.delivered.contains(&n.id)).collect())
    }
    /// Called only after the destination's durable delivery path accepts the same ID.
    pub async fn delivered(&self, notification: &Notification) -> Result<(), String> {
        let registration = &notification.registration;
        let gate = self.gate(&registration.scope, &registration.definition.id);
        let _guard = gate.lock().await;
        let store = self.store(registration);
        let mut ledger = Ledger::load(&store)?;
        ledger.delivered.insert(notification.id.clone());
        ledger.save(&store)
    }
}
impl ResidentHost for Host {
    fn call<'a>(&'a self, scope: &'a HubAccount, request: &'a HomeRequest) -> FutureResult<'a, HomeReply> {
        Box::pin(async move {
            match request {
                HomeRequest::Judge { request } => Ok(HomeReply::Judged { judgment: self.judge(scope, request).await.unwrap_or_else(|reason| Judgment::NotJudged { reason }) }),
                HomeRequest::Inspect { id } => {
                    self.inspect(scope, id).await.map(|sentinel| HomeReply::Summary { sentinel: Box::new(sentinel) })
                }
                HomeRequest::Begin { id, version, owner } => {
                    let gate = self.gate(scope, id);
                    let _guard = gate.lock().await;
                    let current = lifecycle::get(self.records.as_ref(), scope, id)
                        .await
                        .map_err(|e| e.to_string())?
                        .ok_or("sentinel not found")?;
                    if current.1.definition.owner != *owner { return Err("sentinel owner does not match its record".into()); }
                    if current.0 < *version {
                        return Err("sentinel home has not seen this definition version".into());
                    }
                    let summary =
                        self.reconcile_locked(scope, id, None).await?.ok_or("sentinel is assigned to another home")?;
                    Ok(HomeReply::Summary { sentinel: Box::new(summary) })
                }
                // Only the asking account's own: its sentinels live under it.
                HomeRequest::Record { id, generation, held } => {
                    let directory = self
                        .assignment(scope, id)
                        .parent()
                        .expect("assignment directory")
                        .join(generation.to_string());
                    let held = held.clone();
                    let store = crate::state::Store::new(directory.clone());
                    let mut record = tokio::task::spawn_blocking(move || crate::record::read(&directory, held.as_deref()))
                        .await
                        .map_err(|error| error.to_string())??;
                    // How it stands, and when it next looks: the earliest of
                    // its reflexes' times and of the times it wakes its
                    // conversation that are still to come.
                    if !record.unchanged
                        && let Some(state) = store.load()?
                    {
                        let now = self.runtime.now();
                        let due = state.definition.at.iter().copied().filter(|at| *at > now);
                        record.next = state.reflexes.iter().map(|reflex| reflex.at).filter(|at| *at < i64::MAX).chain(due).min();
                        record.summary = Some(Box::new(storage::summary(&state)));
                    }
                    Ok(HomeReply::Record { record: Box::new(record) })
                }
                // Every sentinel the asking account left here, ended or not,
                // with the wakes of its last days.
                HomeRequest::Overview => {
                    let since = self.runtime.now() - OVERVIEW_DAYS * 86_400_000;
                    let scope_dir = self.scope_dir(scope);
                    let watched = tokio::task::spawn_blocking(move || overview(&scope_dir, since))
                        .await
                        .map_err(|error| error.to_string())??;
                    Ok(HomeReply::Overview { watched })
                }
                HomeRequest::Withdraw { id } => {
                    self.reconcile(scope, id).await?;
                    if lifecycle::get(self.records.as_ref(), scope, id).await.map_err(|e| e.to_string())?.is_some() {
                        return Err("sentinel definition still exists".into());
                    }
                    Ok(HomeReply::Withdrawn)
                }
            }
        })
    }
}

/// How many days of wakes the overview of an account's sentinels reads.
const OVERVIEW_DAYS: i64 = 30;

/// Each sentinel under `scope_dir`, every generation of it its home kept,
/// with its wakes since `since`.
fn overview(scope_dir: &std::path::Path, since: i64) -> Result<Vec<Watched>, String> {
    let entries = match std::fs::read_dir(scope_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("reading {}: {error}", scope_dir.display())),
    };
    let mut watched = Vec::new();
    for sentinel in entries.filter_map(Result::ok).filter(|entry| entry.path().is_dir()) {
        let Ok(generations) = std::fs::read_dir(sentinel.path()) else { continue };
        for generation in generations.filter_map(Result::ok) {
            let directory = generation.path();
            if !directory.is_dir() || generation.file_name().to_str().is_none_or(|name| name.parse::<u64>().is_err()) {
                continue;
            }
            // One whose home has kept nothing of it yet has nothing to show.
            let Some(state) = crate::state::Store::new(directory.clone()).load()? else { continue };
            let (wakes, earlier) = crate::record::wakes(&directory, since)?;
            watched.push(Watched { summary: storage::summary(&state), wakes, earlier });
        }
    }
    Ok(watched)
}
