//! Completion letters held while a local `on` is taking the same result.
//! Network receipt is given only after the letter is durable. A separate,
//! durable receipt suppresses every later copy of an end the CLI took.

use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use fastctx_platform::{Permissions, ensure_private_dir, publish};
use fastctx_types::on::OnOrigin;
use fastctx_types::residents::{Delivery, Notice};
use fastctx_types::{Address, JobId, MachineName, NoticeId, ResidentId};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::Node;

type Source = (Address, MachineName);
type Dispatch = (Address, MachineName, Option<NoticeId>);
type Job = (Address, MachineName, JobId, Option<NoticeId>);

#[derive(Default)]
pub(crate) struct Deliveries {
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    starting: HashMap<Dispatch, usize>,
    following: HashMap<Job, usize>,
}

pub(crate) struct Following {
    internal: bool,
    node: Arc<Node>,
    source: Source,
    request: Option<NoticeId>,
    job: Option<JobId>,
}

impl Following {
    /// Register before dispatch: a completion may precede Started.
    pub(crate) fn start(node: &Arc<Node>, origin: &OnOrigin, machine: &MachineName) -> Self {
        let source = (origin.resident.clone(), machine.clone());
        let request = origin.request.clone();
        let dispatch = (source.0.clone(), source.1.clone(), request.clone());
        if origin.sentinel.is_none() {
            *node.on_deliveries.state.lock().expect("never poisoned").starting.entry(dispatch).or_default() += 1;
        }
        Self { internal: origin.sentinel.is_some(), node: node.clone(), source, request, job: None }
    }

    pub(crate) fn started(&mut self, job: &JobId) {
        if self.internal {
            return;
        }
        if self.job.as_ref() == Some(job) {
            return;
        }
        assert!(self.job.is_none(), "one on starts one job");
        let mut state = self.node.on_deliveries.state.lock().expect("never poisoned");
        let key = (self.source.0.clone(), self.source.1.clone(), job.clone(), self.request.clone());
        *state.following.entry(key).or_default() += 1;
        release(&mut state.starting, &(self.source.0.clone(), self.source.1.clone(), self.request.clone()));
        self.job = Some(job.clone());
    }

    pub(crate) fn taken(&self, job: &JobId) -> io::Result<()> {
        if self.internal {
            return Ok(());
        }
        let _state = self.node.on_deliveries.state.lock().expect("never poisoned");
        let path = receipt(
            &self.node,
            &self.source.0.machine,
            &self.source.0.resident,
            &self.source.1,
            job,
            self.request.as_ref(),
        );
        ensure_private_dir(path.parent().expect("receipt directory"))?;
        publish(&path, b"", Permissions::Private)
    }
}

impl Drop for Following {
    fn drop(&mut self) {
        if self.internal {
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
        if self.node.is_me(&self.source.0.machine) {
            crate::received::wake(&self.node, &self.source.0.resident);
        }
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

fn directory(node: &Node, owner: &Address) -> PathBuf {
    node.world.owner(&node.names(), owner).book("on-deliveries")
}

fn receipt(
    node: &Node,
    owner: &MachineName,
    to: &ResidentId,
    machine: &MachineName,
    job: &JobId,
    request: Option<&NoticeId>,
) -> PathBuf {
    let mut parts = vec![owner.key(), to.as_str(), machine.key(), job.as_str()];
    if let Some(request) = request {
        parts.push(request.as_str());
    }
    named(node, &Address { machine: owner.clone(), resident: to.clone() }, &parts).with_extension("taken")
}

fn cancellation(
    node: &Node,
    owner: &MachineName,
    to: &ResidentId,
    machine: &MachineName,
    request: &NoticeId,
) -> PathBuf {
    named(
        node,
        &Address { machine: owner.clone(), resident: to.clone() },
        &[owner.key(), to.as_str(), machine.key(), request.as_str()],
    )
    .with_extension("stopped")
}

pub(crate) fn cancelled(node: &Node, origin: &OnOrigin, machine: &MachineName) -> io::Result<()> {
    if origin.sentinel.is_some() {
        return Ok(());
    }
    let Some(request) = &origin.request else { return Ok(()) };
    let _state = node.on_deliveries.state.lock().expect("never poisoned");
    let path = cancellation(node, &origin.resident.machine, &origin.resident.resident, machine, request);
    ensure_private_dir(path.parent().expect("receipt directory"))?;
    publish(&path, b"", Permissions::Private)
}

fn named(node: &Node, owner: &Address, parts: &[&str]) -> PathBuf {
    let mut hash = Sha256::new();
    for part in parts {
        hash.update(part.as_bytes());
        hash.update([0]);
    }
    let name: String = hash.finalize().iter().map(|byte| format!("{byte:02x}")).collect();
    directory(node, owner).join(name)
}

fn receipt_exists(node: &Node, path: &std::path::Path) -> io::Result<bool> {
    Ok(path.try_exists()?
        || node.world.path().join("on-deliveries").join(path.file_name().expect("receipt name")).try_exists()?)
}

pub(crate) fn stopped(
    node: &Node,
    to: &ResidentId,
    machine: &MachineName,
    request: Option<&NoticeId>,
) -> io::Result<bool> {
    request
        .map(|request| {
            Ok(receipt_exists(node, &cancellation(node, &node.machine(), to, machine, request))?
                || crate::on_stop::pending_cancellation(node, to, machine, request)?)
        })
        .transpose()
        .map(|yes| yes.unwrap_or(false))
}

fn held(state: &State, to: &Address, machine: &MachineName, job: &JobId, request: Option<&NoticeId>) -> bool {
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
pub(crate) fn park(node: &Arc<Node>, to: &ResidentId, delivery: &Delivery) -> io::Result<bool> {
    park_from(node, to, delivery, None)
}

pub(crate) fn park_from(
    node: &Arc<Node>,
    to: &ResidentId,
    delivery: &Delivery,
    sender: Option<&crate::letters::Identity>,
) -> io::Result<bool> {
    let Notice::JobEnded { job, machine, request, taken, .. } = &delivery.notice else { return Ok(false) };
    let _state = node.on_deliveries.state.lock().expect("never poisoned");
    let path = receipt(node, &node.machine(), to, machine, job, request.as_ref());
    if *taken {
        ensure_private_dir(path.parent().expect("receipt directory"))?;
        publish(&path, b"", Permissions::Private)?;
        if !node.residents.knows(to) {
            return Err(io::Error::other("the completion owner is unknown"));
        }
        let directory = node.world.conversation(to);
        let local;
        let sender = match sender {
            Some(sender) => sender,
            None if node.is_me(machine) => {
                local = crate::letters::Identity::local(node, "")?;
                &local
            }
            None => return Err(io::Error::other("a remote completion receipt requires its sender identity")),
        };
        for owner in local_owners(node, to) {
            let id = crate::inbox::end_id(&owner, machine, job, request.as_ref());
            crate::inbox::consume(&directory, &id)?;
            sender.with_id(node, id.as_str()).consume(&directory)?;
        }
        return Ok(true);
    }
    if stopped(node, to, machine, request.as_ref())? {
        return Ok(true);
    }
    for owner in local_owners(node, to) {
        let previous = receipt(node, &owner.machine, to, machine, job, request.as_ref());
        if receipt_exists(node, &previous)? {
            return Ok(true);
        }
        if sender.is_some()
            && let Some(parked) = fastctx_install::renewal::read_json::<Parked>(&previous.with_extension("json"))?
        {
            if parked.to != *to || parked.delivery.notice != delivery.notice {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("the held completion changed: {}", previous.display()),
                ));
            }
            // Preserve beta.2's delivery identity until its original handoff finishes.
            let bytes = serde_json::to_vec(&parked).map_err(io::Error::other)?;
            publish(&previous.with_extension("json"), &bytes, Permissions::Private)?;
            resume(node);
            return Ok(true);
        }
    }
    crate::received::keep(node, to, delivery.clone(), crate::speech::Owed::Working)?;
    Ok(true)
}

fn local_owners(node: &Node, to: &ResidentId) -> Vec<Address> {
    let names = node.machine_names();
    std::iter::once(names.name).chain(names.was).map(|machine| Address { machine, resident: to.clone() }).collect()
}

/// Moves pre-upgrade held letters into the recoverable conversation queues.
pub(crate) fn resume(node: &Arc<Node>) {
    let result = (|| -> io::Result<()> {
        for directory in node.world.book_directories("on-deliveries")? {
            for path in crate::retention::entries(&directory)? {
                if path.extension().is_none_or(|extension| extension != "json")
                    || path.file_name().is_some_and(|name| name.to_string_lossy().starts_with('.'))
                {
                    continue;
                }
                let result = (|| -> io::Result<()> {
                    let Some(parked) = fastctx_install::renewal::read_json::<Parked>(&path)? else { return Ok(()) };
                    crate::received::keep(node, &parked.to, parked.delivery, crate::speech::Owed::Working)?;
                    std::fs::remove_file(&path)
                })();
                if let Err(error) = result {
                    node.log.line(format_args!("migrating held completion {}: {error}", path.display()));
                }
            }
        }
        Ok(())
    })();
    if let Err(error) = result {
        node.log.line(format_args!("finding held completions: {error}"));
    }
}

/// Both the original completion id and its authenticated delivery aliases
/// identify the same end, including records written before machine renaming.
pub(crate) fn completion_ids(node: &Node, to: &ResidentId, delivery: &Delivery) -> io::Result<Vec<NoticeId>> {
    let Notice::JobEnded { machine, job, request, .. } = &delivery.notice else {
        return Err(io::Error::other("a completion identity requires a job end"));
    };
    let mut ids = vec![delivery.id.clone()];
    for owner in local_owners(node, to) {
        let id = crate::inbox::end_id(&owner, machine, job, request.as_ref());
        if let Ok(identity) = crate::letters::Identity::at(node, &machine.into(), id.as_str()) {
            ids.extend(identity.ids().cloned());
        }
        ids.push(id);
    }
    ids.sort();
    ids.dedup();
    Ok(ids)
}

/// A snapshot taken under the same gate that publishes receipts and registers
/// followers. A restarted intermediate node answers from its durable receipt.
pub(crate) fn status(
    node: &Node,
    owner: &Address,
    machine: &MachineName,
    job: &JobId,
    request: Option<&NoticeId>,
) -> io::Result<(bool, bool)> {
    let state = node.on_deliveries.state.lock().expect("never poisoned");
    let taken = receipt_exists(node, &receipt(node, &owner.machine, &owner.resident, machine, job, request))?;
    Ok((taken, held(&state, owner, machine, job, request)))
}
