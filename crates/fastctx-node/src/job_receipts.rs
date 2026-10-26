//! One completion handover has two facts: the CLI consumed its end, and the
//! owning session received the enclosing answer. Only the second starts age.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use fastctx_hands::run::record;
use fastctx_platform::{Permissions, ensure_private_dir, publish_new};
use fastctx_types::access::{Acting, HubAccount, Written};
use fastctx_types::on::OnOrigin;
use fastctx_types::remote::RemoteReply;
use fastctx_types::residents::{Delivery, Notice};
use fastctx_types::run::{CompletedJob, JobDelivery};
use fastctx_types::{Address, HubName, JobId, MachineName, NoticeId};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::Node;

#[derive(Serialize, Deserialize)]
struct Sending {
    owner: Address,
    machine: MachineName,
    acting: Acting,
    delivery: Option<JobDelivery>,
    #[serde(default)]
    consumption: Option<Delivery>,
    #[serde(default)]
    origin: Option<OnOrigin>,
    #[serde(default)]
    hub: Option<HubName>,
    #[serde(default)]
    through_exit: bool,
}

#[derive(Clone, Serialize, Deserialize)]
struct Child {
    completed: CompletedJob,
    destination: Written,
    acting: Acting,
    origin: Option<OnOrigin>,
    copied: bool,
    #[serde(default)]
    through_exit: bool,
}

pub(crate) struct OnCompletion<'a> {
    pub(crate) job: &'a JobId,
    pub(crate) consumed: &'a [NoticeId],
    pub(crate) through_exit: bool,
}

#[derive(Serialize, Deserialize)]
struct ExitReceipt {
    children: Vec<Child>,
    consumed: Vec<NoticeId>,
}

/// Durable before an inbox entry or the parent job can disappear. A remote
/// node receives the original host time, even after an offline interval.
pub(crate) fn confirmed(
    node: &Node,
    owner: Address,
    machine: MachineName,
    delivery: JobDelivery,
    acting: Option<&Acting>,
) -> io::Result<()> {
    let acting = match acting {
        Some(acting) => acting.clone(),
        None => crate::authorization::resident(node, &owner.resident).map_err(io::Error::other)?,
    };
    let gate = node.disk.lock();
    confirmed_under(&gate, node, owner, machine, delivery, Some(&acting))
}

/// `confirmed` by one already holding the disk's lock (`_gate`), as one
/// that also holds the records' lock must: the disk's comes first.
pub(crate) fn confirmed_under(
    _gate: &fastctx_platform::doing::SyncMutexGuard<'_, ()>,
    node: &Node,
    owner: Address,
    machine: MachineName,
    delivery: JobDelivery,
    acting: Option<&Acting>,
) -> io::Result<()> {
    crate::run_receipts::confirmed(node, &owner, &machine, &delivery)?;
    let dispatched = crate::here::get_dispatched(node, &owner.resident, &delivery.job);
    let destination = dispatched.map(|(destination, _)| destination).unwrap_or_else(|| machine.clone().into());
    if node.is_here(&destination) {
        received_here(node, &owner, &delivery, None, false, None, &mut HashSet::new())?;
    } else {
        let hub = destination.hub;
        let acting =
            acting.cloned().ok_or_else(|| io::Error::other("the completion receipt has no account identity"))?;
        let key = (owner.resident.clone(), delivery.job.clone());
        enqueue(
            node,
            Sending {
                owner,
                machine,
                acting,
                delivery: Some(delivery),
                consumption: None,
                origin: None,
                hub,
                through_exit: false,
            },
        )?;
        crate::here::take_dispatched(node, &key.0, &key.1);
    }
    node.disk_changed.notify_one();
    Ok(())
}

fn enqueue(node: &Node, sending: Sending) -> io::Result<()> {
    let request = sending.origin.as_ref().and_then(|origin| origin.request.as_ref()).map_or("", |id| id.as_str());
    let (job, output, phase) = match (&sending.delivery, &sending.consumption) {
        (Some(delivery), None) => (delivery.job.as_str(), delivery.output.as_str(), "delivered"),
        (None, Some(delivery)) => (delivery.id.as_str(), "", "consumed"),
        _ => return Err(io::Error::other("invalid completion receipt phase")),
    };
    let key = crate::speech::fixed_id(
        "job-delivery",
        &[
            sending.owner.machine.key(),
            sending.owner.resident.as_str(),
            sending.machine.key(),
            job,
            output,
            request,
            phase,
            if sending.through_exit { "exit" } else { "end" },
        ],
    );
    let directory = node.world.owner(&node.names(), &sending.owner).book("job-deliveries");
    ensure_private_dir(&directory)?;
    queue(&directory.join(format!("{key}.json")), sending)?;
    node.receipts_changed.notify_one();
    Ok(())
}

fn queue(path: &Path, sending: Sending) -> io::Result<()> {
    if let Some(mut prior) = fastctx_install::renewal::read_json::<Sending>(path)? {
        if prior.owner != sending.owner
            || prior.machine != sending.machine
            || prior.origin != sending.origin
            || prior.through_exit != sending.through_exit
            || (prior.hub.is_some() && sending.hub.is_some() && prior.hub != sending.hub)
        {
            return Err(io::Error::other("job delivery queue identity changed"));
        }
        if prior.hub.is_none() {
            prior.hub.clone_from(&sending.hub);
        }
        match (&mut prior.delivery, &sending.delivery, &prior.consumption, &sending.consumption) {
            (Some(prior), Some(next), None, None) if prior.job == next.job && prior.output == next.output => {
                prior.shown_whole |= next.shown_whole
            }
            (None, None, Some(prior), Some(next)) if prior.id == next.id && prior.notice == next.notice => {}
            _ => return Err(io::Error::other("job delivery queue identity changed")),
        }
        fastctx_install::renewal::write_json(path, &prior)
    } else {
        create(path, &sending)
    }
}

pub(crate) fn received(node: &Node, owner: &Address, delivery: &JobDelivery) -> io::Result<()> {
    received_from(node, owner, delivery, None, false, None)
}

pub(crate) fn received_from(
    node: &Node,
    owner: &Address,
    delivery: &JobDelivery,
    origin: Option<&OnOrigin>,
    through_exit: bool,
    account: Option<&HubAccount>,
) -> io::Result<()> {
    let _gate = node.disk.lock();
    received_here(node, owner, delivery, origin, through_exit, account, &mut HashSet::new())
}

fn received_here(
    node: &Node,
    owner: &Address,
    delivery: &JobDelivery,
    origin: Option<&OnOrigin>,
    through_exit: bool,
    account: Option<&HubAccount>,
    seen: &mut HashSet<(PathBuf, bool)>,
) -> io::Result<()> {
    let paths = if let Some(origin) = origin {
        vec![node.world.job_record(&node.names(), origin.town.as_ref(), &owner.clone().into(), &delivery.job)]
    } else {
        let mut paths = Vec::new();
        for parent in node.world.job_parents()? {
            for directory in crate::retention::entries(&parent)? {
                paths.push(directory.join("jobs").join(format!("{}.json", delivery.job)));
            }
        }
        paths
    };
    for path in paths {
        let job = match record::read(&path) {
            Ok(job) => job,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        if job.owner != *owner
            || job.id != delivery.job
            || (!delivery.output.is_empty() && job.output != delivery.output)
            || (delivery.output.is_empty() && origin.is_none())
            || origin
                .is_some_and(|origin| job.on.as_ref().and_then(|on| on.request.as_ref()) != origin.request.as_ref())
        {
            continue;
        }
        if let (Some(expected), Some(account)) = (&job.account, account)
            && expected != account
        {
            return Err(io::Error::other("the end receipt is for another account"));
        }
        if let Some(origin) = origin
            && job.on.as_ref().and_then(|on| on.account.as_ref()) != origin.account.as_ref()
        {
            return Err(io::Error::other("the end receipt is for another account"));
        }
        if !seen.insert((path.clone(), through_exit)) {
            return Ok(());
        }
        if !through_exit && job.end.is_none() {
            return Err(io::Error::other("a running job cannot have a delivered end"));
        }
        if through_exit
            && (origin.is_none()
                || job.exit.is_none()
                || (job.output_drained.is_none() && job.output_ended_at.is_none()))
        {
            return Err(io::Error::other("the on shell has not delivered its complete output"));
        }
        let marker = path.with_extension("delivered");
        let first = if through_exit { None } else { fastctx_install::renewal::read_json::<JobDelivery>(&marker)? };
        let mut confirmed = first.clone().unwrap_or_else(|| delivery.clone());
        confirmed.output.clone_from(&job.output);
        confirmed.shown_whole |= delivery.shown_whole;
        let children = if job.output_lost.is_some() || job.stderr_lost.is_some() {
            node.log.line(format_args!("{} lost output; its answer cannot confirm its children's ends", job.id));
            None
        } else if through_exit {
            Some(
                fastctx_install::renewal::read_json::<ExitReceipt>(&path.with_extension("exit-receipt"))?
                    .ok_or_else(|| io::Error::other("the partial on end has no frozen child receipts"))?
                    .children,
            )
        } else {
            fastctx_install::renewal::read_json::<Vec<Child>>(&path.with_extension("waited"))?
        };
        if let Some(children) = children {
            for child in children {
                let receipt = JobDelivery {
                    job: child.completed.job,
                    output: child.completed.output,
                    at: confirmed.at,
                    shown_whole: child.copied,
                };
                if node.is_here(&child.destination) {
                    received_here(node, owner, &receipt, child.origin.as_ref(), child.through_exit, account, seen)?;
                } else {
                    enqueue(
                        node,
                        Sending {
                            owner: owner.clone(),
                            machine: child.destination.name,
                            acting: child.acting,
                            delivery: Some(receipt),
                            consumption: None,
                            origin: child.origin,
                            hub: child.destination.hub,
                            through_exit: child.through_exit,
                        },
                    )?;
                }
            }
        }
        if through_exit {
            return Ok(());
        }
        if first.as_ref().is_some_and(|prior| prior.shown_whole != confirmed.shown_whole) {
            fastctx_install::renewal::write_json(&marker, &confirmed)?;
        } else {
            create(&marker, &confirmed)?;
        }
        crate::completion_debt::paid(&path)?;
        if confirmed.shown_whole {
            node.disk_due.lock().expect("never poisoned").insert(path.clone());
        }
        let directory = path.parent().and_then(Path::parent).expect("job owner directory");
        let latest = directory.join("artifacts-delivered.json");
        if fastctx_install::renewal::read_json::<Timestamp>(&latest)?.is_none_or(|prior| prior < confirmed.at) {
            fastctx_install::renewal::write_json(&latest, &confirmed.at)?;
        }
        node.disk_changed.notify_one();
        return Ok(());
    }
    Ok(())
}

fn bind(path: &Path, child: Child) -> io::Result<()> {
    let marker = path.with_extension("waited");
    let mut children = fastctx_install::renewal::read_json::<Vec<Child>>(&marker)?.unwrap_or_default();
    if !children.iter().any(|prior| {
        prior.completed == child.completed && prior.origin == child.origin && prior.through_exit == child.through_exit
    }) {
        children.push(child);
        fastctx_install::renewal::write_json(&marker, &children)?;
    }
    Ok(())
}

fn inherit_consumed(parent: &Path, consumed: &[NoticeId]) -> io::Result<()> {
    let file = parent.with_extension("consumed");
    let mut prior = fastctx_install::renewal::read_json::<Vec<NoticeId>>(&file)?.unwrap_or_default();
    for receipt in consumed {
        if !prior.contains(receipt) {
            prior.push(receipt.clone());
        }
    }
    if !prior.is_empty() {
        fastctx_install::renewal::write_json(&file, &prior)?;
    }
    Ok(())
}

/// The held records are the ends actually written before TAKEN. Another
/// resident's wait neither consumes the owner's notice nor starts its clock.
pub(crate) fn wait_taken(
    node: &Node,
    caller: Option<&Address>,
    parent: Option<&JobId>,
    jobs: &[CompletedJob],
) -> io::Result<()> {
    let Some(caller) = caller else { return Ok(()) };
    let _gate = node.disk.lock();
    let parent = parent
        .map(|parent| {
            let path = crate::stop::find(&node.world, parent)?
                .ok_or_else(|| io::Error::other("the enclosing wait job disappeared"))?;
            let record = record::read(&path)?;
            if record.owner != *caller {
                return Err(io::Error::other("wait does not belong to the enclosing job"));
            }
            Ok(path)
        })
        .transpose()?;
    for completed in jobs {
        let path = crate::stop::find(&node.world, &completed.job)?
            .ok_or_else(|| io::Error::other("waited job disappeared"))?;
        let record = record::read(&path)?;
        if record.machine != completed.machine || record.output != completed.output || record.end.is_none() {
            return Err(io::Error::other("wait receipt does not match the ended job"));
        }
        if record.owner != *caller {
            continue;
        }
        if let Some(parent) = &parent {
            bind(
                parent,
                Child {
                    completed: completed.clone(),
                    destination: (&completed.machine).into(),
                    acting: Acting::default(),
                    origin: None,
                    copied: false,
                    through_exit: false,
                },
            )?;
            let id = crate::inbox::job_notice_id(&record);
            let identity = crate::letters::Identity::local(node, id.as_str())?;
            let consumed: Vec<_> = std::iter::once(id).chain(identity.ids().cloned()).collect();
            inherit_consumed(parent, &consumed)?;
        } else {
            received_here(
                node,
                caller,
                &JobDelivery {
                    job: completed.job.clone(),
                    output: completed.output.clone(),
                    at: Timestamp::now(),
                    shown_whole: false,
                },
                None,
                false,
                None,
                &mut HashSet::new(),
            )?;
        }
    }
    Ok(())
}

/// Called only after the on CLI acknowledged a complete end. Its parent
/// will deliver this proof transitively when the final host receives it.
pub(crate) fn on_taken(
    node: &Node,
    parent: Option<&Path>,
    origin: &OnOrigin,
    destination: &Written,
    acting: &Acting,
    completion: OnCompletion<'_>,
) -> io::Result<()> {
    let _gate = node.disk.lock();
    let child = Child {
        completed: CompletedJob {
            machine: destination.name.clone(),
            job: completion.job.clone(),
            output: String::new(),
        },
        destination: destination.clone(),
        acting: acting.clone(),
        origin: Some(origin.clone()),
        copied: !completion.through_exit,
        through_exit: completion.through_exit,
    };
    if let Some(parent) = parent {
        if record::read(parent)?.owner != origin.resident {
            return Err(io::Error::other("the on belongs to another enclosing job"));
        }
        bind(parent, child)?;
        let mut consumed = completion.consumed.to_vec();
        if !completion.through_exit {
            let id = crate::inbox::end_id(&origin.resident, &destination.name, completion.job, origin.request.as_ref());
            let identity = crate::letters::Identity::at(node, destination, id.as_str())?;
            consumed.extend(identity.ids().cloned());
            consumed.push(id);
        }
        inherit_consumed(parent, &consumed)?;
    } else {
        let receipt = JobDelivery {
            job: completion.job.clone(),
            output: String::new(),
            at: Timestamp::now(),
            shown_whole: !completion.through_exit,
        };
        if node.is_here(destination) {
            received_here(
                node,
                &origin.resident,
                &receipt,
                Some(origin),
                completion.through_exit,
                None,
                &mut HashSet::new(),
            )?;
        } else {
            enqueue(
                node,
                Sending {
                    owner: origin.resident.clone(),
                    machine: destination.name.clone(),
                    acting: acting.clone(),
                    delivery: Some(receipt),
                    consumption: None,
                    origin: Some(origin.clone()),
                    hub: destination.hub.clone(),
                    through_exit: completion.through_exit,
                },
            )?;
        }
        node.disk_changed.notify_one();
    }
    Ok(())
}

pub(crate) fn apply_consumed(
    directory: &fastctx_types::layout::ConversationDir,
    consumed: &[NoticeId],
) -> io::Result<()> {
    for id in consumed {
        crate::inbox::consume(directory, id)?;
    }
    Ok(())
}

/// Only this enclosing job's causal children enter its response. Historical
/// wait receipts are not an ever-growing payload of every later run.
pub(crate) fn snapshot(node: &Node, path: &Path) -> io::Result<Vec<NoticeId>> {
    snapshot_through(node, path, false)
}

pub(crate) fn snapshot_through(node: &Node, path: &Path, through_exit: bool) -> io::Result<Vec<NoticeId>> {
    let _gate = node.disk.lock();
    if through_exit
        && let Some(receipt) = fastctx_install::renewal::read_json::<ExitReceipt>(&path.with_extension("exit-receipt"))?
    {
        return Ok(receipt.consumed);
    }
    let mut receipts = Vec::new();
    snapshot_here(node, path, &mut HashSet::new(), &mut receipts)?;
    if through_exit {
        let children =
            fastctx_install::renewal::read_json::<Vec<Child>>(&path.with_extension("waited"))?.unwrap_or_default();
        create(&path.with_extension("exit-receipt"), &ExitReceipt { children, consumed: receipts.clone() })?;
    }
    Ok(receipts)
}

fn snapshot_here(
    node: &Node,
    path: &Path,
    seen: &mut HashSet<PathBuf>,
    receipts: &mut Vec<NoticeId>,
) -> io::Result<()> {
    if !seen.insert(path.to_path_buf()) {
        return Ok(());
    }
    let record = match record::read(path) {
        Ok(record) => record,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if let Some(consumed) = fastctx_install::renewal::read_json::<Vec<NoticeId>>(&path.with_extension("consumed"))? {
        for receipt in consumed {
            if !receipts.contains(&receipt) {
                receipts.push(receipt);
            }
        }
    }
    if let Some(children) = fastctx_install::renewal::read_json::<Vec<Child>>(&path.with_extension("waited"))? {
        for child in children {
            if !node.is_here(&child.destination) {
                continue;
            }
            for parent in node.world.job_parents()? {
                for directory in crate::retention::entries(&parent)? {
                    let path = directory.join("jobs").join(format!("{}.json", child.completed.job));
                    let target = match record::read(&path) {
                        Ok(record) => record,
                        Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                        Err(error) => return Err(error),
                    };
                    if target.owner != record.owner
                        || (!child.completed.output.is_empty() && target.output != child.completed.output)
                        || child.origin.as_ref().is_some_and(|origin| {
                            target.on.as_ref().and_then(|on| on.request.as_ref()) != origin.request.as_ref()
                        })
                    {
                        continue;
                    }
                    if child.through_exit {
                        let receipt = fastctx_install::renewal::read_json::<ExitReceipt>(
                            &path.with_extension("exit-receipt"),
                        )?
                        .ok_or_else(|| io::Error::other("the partial on end has no frozen consumption receipts"))?;
                        for id in receipt.consumed {
                            if !receipts.contains(&id) {
                                receipts.push(id);
                            }
                        }
                        continue;
                    }
                    let marker = path.with_extension("wait-taken");
                    match fs::read_to_string(&marker) {
                        Ok(id) => {
                            let id = if id.is_empty() {
                                crate::inbox::job_notice_id(&target)
                            } else {
                                id.parse().map_err(io::Error::other)?
                            };
                            let identity = crate::letters::Identity::local(node, id.as_str())?;
                            for receipt in std::iter::once(id).chain(identity.ids().cloned()) {
                                if !receipts.contains(&receipt) {
                                    receipts.push(receipt);
                                }
                            }
                        }
                        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                        Err(error) => return Err(error),
                    }
                    snapshot_here(node, &path, seen, receipts)?;
                }
            }
        }
    }
    Ok(())
}

/// A consumed end is durable even while its owner is offline. This phase
/// suppresses notification; it contains no retention timestamp.
pub(crate) fn consumed(node: &Node, owner: &Address, acting: &Acting, id: NoticeId, notice: Notice) -> io::Result<()> {
    let _gate = node.disk.lock();
    enqueue(
        node,
        Sending {
            owner: owner.clone(),
            machine: owner.machine.clone(),
            acting: acting.clone(),
            delivery: None,
            consumption: Some(Delivery { id, at: Timestamp::now(), notice }),
            origin: None,
            hub: None,
            through_exit: false,
        },
    )?;
    node.disk_changed.notify_one();
    Ok(())
}

/// Receipt forwarding has its own retry loop; artifact maintenance neither
/// paces it nor waits behind a remote receipt.
pub(crate) fn start(node: &std::sync::Arc<Node>) {
    let node = node.clone();
    tokio::spawn(async move {
        let mut again = crate::again::Again::default();
        let mut ways = node.ways.subscribe();
        let mut previous = None;
        loop {
            let changed = node.receipts_changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            ways.borrow_and_update();
            let pending = match forward(&node).await {
                Ok(pending) => {
                    previous = None;
                    pending
                }
                Err(error) => {
                    let reason = error.to_string();
                    if previous.as_ref() != Some(&reason) {
                        node.log.line(format_args!("forwarding job receipts: {reason}"));
                        previous = Some(reason);
                    }
                    true
                }
            };
            if !pending {
                again.reset();
            }
            let pause = if pending { again.next() } else { std::time::Duration::from_secs(3600) };
            tokio::select! {
                () = tokio::time::sleep(pause) => {}
                () = &mut changed => {}
                _ = ways.changed() => {}
            }
        }
    });
}

async fn forward(node: &std::sync::Arc<Node>) -> io::Result<bool> {
    let mut pending = false;
    for directory in node.world.book_directories("job-deliveries")? {
        let paths = match crate::retention::entries(&directory) {
            Ok(paths) => paths,
            Err(error) => {
                pending = true;
                crate::obligations::retry(node, &directory.join("directory"), &error.to_string())?;
                continue;
            }
        };
        for path in paths {
            if path.extension().is_none_or(|extension| extension != "json")
                || path.file_name().is_some_and(|name| name.to_string_lossy().starts_with('.'))
            {
                continue;
            }
            match forward_one(node, &path).await {
                Ok(done) => pending |= !done,
                Err(error) => {
                    pending = true;
                    crate::obligations::retry(node, &path, &error.to_string())?;
                }
            }
        }
    }
    Ok(pending)
}

async fn forward_one(node: &std::sync::Arc<Node>, path: &Path) -> io::Result<bool> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(true),
        Err(error) => return Err(error),
    };
    let sending: Sending = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
    let destination = Written { name: sending.machine.clone(), hub: sending.hub.clone() };
    if crate::obligations::finished(path, &bytes)? {
        let _gate = node.disk.lock();
        return crate::obligations::finish(
            node,
            path,
            &bytes,
            crate::obligations::Outcome::Superseded,
            "duplicate of an already settled receipt",
        );
    }
    let impossible = if sending.delivery.is_some() {
        crate::obligations::control_impossible(node, &sending.acting, &destination)?
    } else {
        crate::obligations::impossible(node, &sending.acting, &destination)?
    };
    if let Some(reason) = impossible {
        let _gate = node.disk.lock();
        return crate::obligations::finish(node, path, &bytes, crate::obligations::Outcome::Impossible, &reason);
    }
    if !crate::facts::completion_receipts(node, &destination) {
        let whole = sending
            .delivery
            .as_ref()
            .filter(|delivery| delivery.shown_whole)
            .filter(|_| !sending.through_exit && sending.origin.is_none());
        if let Some(delivery) = whole {
            let call = fastctx_types::remote::RemoteCall::Forget {
                resident: sending.owner.resident.clone(),
                town: None,
                job: delivery.job.clone(),
                delivery: None,
                origin: None,
                through_exit: false,
            };
            match node.call(&sending.acting, &destination, fastctx_types::access::Purpose::Hands, call).await {
                Ok(Ok(RemoteReply::Set)) => {}
                other => {
                    crate::obligations::retry(node, path, &format!("legacy whole-answer cleanup: {other:?}"))?;
                    return Ok(false);
                }
            }
        }
        let _gate = node.disk.lock();
        return crate::obligations::finish(
            node,
            path,
            &bytes,
            crate::obligations::Outcome::Superseded,
            "the legacy peer does not use separate completion receipts",
        );
    }
    let result = if let Some(delivery) = sending.delivery {
        let command = fastctx_types::delivery_control::ControlLetter::CompletionReceipt {
            owner: sending.owner,
            delivery,
            origin: sending.origin,
            through_exit: sending.through_exit,
        };
        if crate::facts::control_letters(node, &destination) {
            let Some(result) = crate::control_posts::send(node, path, &bytes, &sending.acting, &destination, command)?
            else {
                return Ok(false);
            };
            result
        } else {
            // An old peer has no receipt-letter format. Keep its existing
            // call durable here without a capability preflight.
            match tokio::time::timeout(
                std::time::Duration::from_secs(10),
                node.call(&sending.acting, &destination, fastctx_types::access::Purpose::Hands, command.call()),
            )
            .await
            {
                Ok(Ok(Ok(RemoteReply::Set))) => Ok(()),
                Ok(Ok(Err(error))) => Err(error),
                Ok(other) => Err(fastctx_types::ipc::Failure::new(format!("legacy receipt is unanswered: {other:?}"))),
                Err(_) => Err(fastctx_types::ipc::Failure::new("legacy receipt did not answer within 10 seconds")),
            }
        }
    } else if let Some(consumption) = sending.consumption {
        crate::speech::tell(
            node,
            &sending.acting,
            &sending.owner,
            consumption.id,
            consumption.notice,
            crate::speech::Owed::Working,
        )
        .await
        .map(|_| ())
    } else {
        return Err(io::Error::other("invalid completion receipt phase"));
    };
    let (outcome, reason) = match result {
        Ok(()) => (crate::obligations::Outcome::Delivered, "receipt delivered".to_string()),
        Err(error) if crate::obligations::permanent(&error) => (crate::obligations::Outcome::Impossible, error.message),
        Err(error) => {
            crate::obligations::retry(node, path, &error.message)?;
            return Ok(false);
        }
    };
    let _gate = node.disk.lock();
    let done = crate::obligations::finish(node, path, &bytes, outcome, &reason)?;
    if !done {
        node.receipts_changed.notify_one();
    }
    Ok(done)
}

fn create(path: &Path, value: &impl Serialize) -> io::Result<()> {
    match publish_new(path, &serde_json::to_vec(value).map_err(io::Error::other)?, Permissions::Private) {
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        result => result,
    }
}
