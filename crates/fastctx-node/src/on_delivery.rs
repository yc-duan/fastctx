//! Completion letters held while a local `on` is taking the same result.
//! Network receipt is given only after the letter is durable. A separate,
//! durable receipt suppresses every later copy of an end the CLI took.

use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use fastctx_platform::{Permissions, ensure_private_dir, publish, publish_new};
use fastctx_types::on::OnOrigin;
use fastctx_types::residents::{Delivery, Notice};
use fastctx_types::{JobId, MachineName, NoticeId, ResidentId};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::Node;

type Source = (ResidentId, MachineName);
type Dispatch = (ResidentId, MachineName, Option<NoticeId>);
type Job = (ResidentId, MachineName, JobId, Option<NoticeId>);

#[derive(Default)]
pub(crate) struct Deliveries {
    state: Mutex<State>,
    draining: tokio::sync::Mutex<()>,
}

#[derive(Default)]
struct State {
    starting: HashMap<Dispatch, usize>,
    following: HashMap<Job, usize>,
}

pub(crate) struct Following {
    node: Arc<Node>,
    source: Source,
    owner_machine: MachineName,
    local: bool,
    request: Option<NoticeId>,
    job: Option<JobId>,
}

impl Following {
    /// Register before dispatch: a completion may precede Started.
    pub(crate) fn start(node: &Arc<Node>, origin: &OnOrigin, machine: &MachineName) -> Self {
        let source = (origin.resident.resident.clone(), machine.clone());
        let request = origin.request.clone();
        let dispatch = (source.0.clone(), source.1.clone(), request.clone());
        let local = origin.resident.machine == node.machine();
        if local {
            *node.on_deliveries.state.lock().expect("never poisoned").starting.entry(dispatch).or_default() += 1;
        }
        Self { node: node.clone(), source, owner_machine: origin.resident.machine.clone(), local, request, job: None }
    }

    pub(crate) fn started(&mut self, job: &JobId) {
        if self.job.as_ref() == Some(job) {
            return;
        }
        assert!(self.job.is_none(), "one on starts one job");
        if !self.local {
            self.job = Some(job.clone());
            return;
        }
        let mut state = self.node.on_deliveries.state.lock().expect("never poisoned");
        let key = (self.source.0.clone(), self.source.1.clone(), job.clone(), self.request.clone());
        *state.following.entry(key).or_default() += 1;
        release(&mut state.starting, &(self.source.0.clone(), self.source.1.clone(), self.request.clone()));
        self.job = Some(job.clone());
    }

    pub(crate) fn taken(&self, job: &JobId) -> io::Result<()> {
        let _state = self.node.on_deliveries.state.lock().expect("never poisoned");
        let path = receipt(&self.node, &self.owner_machine, &self.source.0, &self.source.1, job, self.request.as_ref());
        ensure_private_dir(path.parent().expect("receipt directory"))?;
        publish(&path, b"", Permissions::Private)
    }
}

impl Drop for Following {
    fn drop(&mut self) {
        if !self.local {
            return;
        }
        {
            let mut state = self.node.on_deliveries.state.lock().expect("never poisoned");
            match &self.job {
                Some(job) => release(
                    &mut state.following,
                    &(self.source.0.clone(), self.source.1.clone(), job.clone(), self.request.clone()),
                ),
                None => {
                    release(&mut state.starting, &(self.source.0.clone(), self.source.1.clone(), self.request.clone()))
                }
            }
        }
        resume(&self.node);
    }
}

fn release<K: Eq + std::hash::Hash>(counts: &mut HashMap<K, usize>, key: &K) {
    let count = counts.get_mut(key).expect("registered follower");
    *count -= 1;
    if *count == 0 {
        counts.remove(key);
    }
}

#[derive(Serialize, Deserialize)]
struct Parked {
    to: ResidentId,
    delivery: Delivery,
}

fn directory(node: &Node) -> PathBuf {
    node.world.path().join("on-deliveries")
}

fn receipt(
    node: &Node,
    owner: &MachineName,
    to: &ResidentId,
    machine: &MachineName,
    job: &JobId,
    request: Option<&NoticeId>,
) -> PathBuf {
    let mut parts = vec![owner.as_str(), to.as_str(), machine.as_str(), job.as_str()];
    if let Some(request) = request {
        parts.push(request.as_str());
    }
    named(node, &parts).with_extension("taken")
}

fn cancellation(
    node: &Node,
    owner: &MachineName,
    to: &ResidentId,
    machine: &MachineName,
    request: &NoticeId,
) -> PathBuf {
    named(node, &[owner.as_str(), to.as_str(), machine.as_str(), request.as_str()]).with_extension("stopped")
}

/// Keeps the caller's outstanding request before dispatch. A completion
/// after restart can restore its reaching guard without depending on an
/// unrelated read of the resident's whole action history succeeding.
pub(crate) fn keep_request(node: &Node, origin: &OnOrigin, machine: &MachineName) -> io::Result<()> {
    if origin.resident.machine != node.machine() {
        return Ok(());
    }
    let request = origin.request.as_ref().expect("new on has a request identity");
    let path =
        cancellation(node, &node.machine(), &origin.resident.resident, machine, request).with_extension("requested");
    ensure_private_dir(&directory(node))?;
    publish(&path, b"", Permissions::Private)
}

pub(crate) fn requested_here(
    node: &Node,
    to: &ResidentId,
    machine: &MachineName,
    request: &NoticeId,
) -> io::Result<bool> {
    cancellation(node, &node.machine(), to, machine, request).with_extension("requested").try_exists()
}

pub(crate) fn cancelled(node: &Node, origin: &OnOrigin, machine: &MachineName) -> io::Result<()> {
    let Some(request) = &origin.request else { return Ok(()) };
    let _state = node.on_deliveries.state.lock().expect("never poisoned");
    let path = cancellation(node, &origin.resident.machine, &origin.resident.resident, machine, request);
    ensure_private_dir(&directory(node))?;
    publish(&path, b"", Permissions::Private)
}

fn named(node: &Node, parts: &[&str]) -> PathBuf {
    let mut hash = Sha256::new();
    for part in parts {
        hash.update(part.as_bytes());
        hash.update([0]);
    }
    let name: String = hash.finalize().iter().map(|byte| format!("{byte:02x}")).collect();
    directory(node).join(name)
}

fn stopped(node: &Node, to: &ResidentId, machine: &MachineName, request: Option<&NoticeId>) -> io::Result<bool> {
    request
        .map(|request| {
            Ok(cancellation(node, &node.machine(), to, machine, request).try_exists()?
                || crate::on_stop::pending_cancellation(node, to, machine, request)?)
        })
        .transpose()
        .map(|yes| yes.unwrap_or(false))
}

fn held(state: &State, to: &ResidentId, machine: &MachineName, job: &JobId, request: Option<&NoticeId>) -> bool {
    let starting = match request {
        Some(request) => state.starting.contains_key(&(to.clone(), machine.clone(), Some(request.clone()))),
        None => state.starting.keys().any(|(resident, source, _)| resident == to && source == machine),
    };
    let following = match request {
        Some(request) => {
            state.following.contains_key(&(to.clone(), machine.clone(), job.clone(), Some(request.clone())))
        }
        None => state
            .following
            .keys()
            .any(|(resident, source, followed, _)| resident == to && source == machine && followed == job),
    };
    starting || following
}

/// Called after admission, before ordinary inbox delivery. A successful
/// return may be acknowledged to the sender, even if the node now stops.
pub(crate) fn park(node: &Node, to: &ResidentId, delivery: &Delivery) -> io::Result<bool> {
    let Notice::JobEnded { job, machine, request, .. } = &delivery.notice else { return Ok(false) };
    let state = node.on_deliveries.state.lock().expect("never poisoned");
    let path = receipt(node, &node.machine(), to, machine, job, request.as_ref());
    if path.try_exists()? || stopped(node, to, machine, request.as_ref())? {
        return Ok(true);
    }
    if !held(&state, to, machine, job, request.as_ref()) {
        return Ok(false);
    }
    ensure_private_dir(&directory(node))?;
    let parked = Parked { to: to.clone(), delivery: delivery.clone() };
    let bytes = serde_json::to_vec(&parked).map_err(io::Error::other)?;
    match publish_new(&path.with_extension("json"), &bytes, Permissions::Private) {
        Err(error) if error.kind() != io::ErrorKind::AlreadyExists => Err(error),
        _ => Ok(true),
    }
}

/// Startup and follower departure both release durable, unclaimed letters.
pub(crate) fn resume(node: &Arc<Node>) {
    let node = node.clone();
    tokio::spawn(async move {
        let _draining = node.on_deliveries.draining.lock().await;
        let mut delay = crate::on::Backoff::default();
        loop {
            match drain(&node).await {
                Ok(()) => return,
                Err(error) => node.log.line(format_args!("delivering held on completions: {error}")),
            }
            tokio::time::sleep(delay.next()).await;
        }
    });
}

async fn drain(node: &Arc<Node>) -> io::Result<()> {
    let entries = match std::fs::read_dir(directory(node)) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let mut failed = false;
    for entry in entries {
        let path = match entry {
            Ok(entry) => entry.path(),
            Err(error) => {
                failed = true;
                node.log.line(format_args!("reading held on completions: {error}"));
                continue;
            }
        };
        if path.extension().is_none_or(|extension| extension != "json")
            || path.file_name().is_some_and(|name| name.to_string_lossy().starts_with('.'))
        {
            continue;
        }
        if let Err(error) = drain_one(node, &path).await {
            failed = true;
            node.log.line(format_args!("{}: {error}", path.display()));
        }
    }
    if failed { Err(io::Error::other("some on completions remain pending; see the individual errors")) } else { Ok(()) }
}

async fn drain_one(node: &Arc<Node>, path: &std::path::Path) -> io::Result<()> {
    let parked: Parked = serde_json::from_slice(&std::fs::read(path)?).map_err(io::Error::other)?;
    let Notice::JobEnded { job, machine, request, .. } = &parked.delivery.notice else {
        return Err(io::Error::other(format!("{} is not a completion", path.display())));
    };
    let taken = {
        let state = node.on_deliveries.state.lock().expect("never poisoned");
        if held(&state, &parked.to, machine, job, request.as_ref()) {
            return Ok(());
        }
        receipt(node, &node.machine(), &parked.to, machine, job, request.as_ref()).try_exists()?
            || stopped(node, &parked.to, machine, request.as_ref())?
    };
    if !taken {
        // The inbox takes custody before the parked copy is removed.
        crate::speech::tell_here(node, &parked.to, parked.delivery, crate::speech::Owed::Working)
            .await
            .map_err(|failure| io::Error::other(failure.message))?;
    }
    std::fs::remove_file(path)?;
    Ok(())
}
