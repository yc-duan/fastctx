//! One completion handover has two facts: the CLI consumed its end, and the
//! owning session received the enclosing answer. Only the second starts age.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use fastctx_hands::run::record;
use fastctx_platform::{Permissions, ensure_private_dir, publish_new};
use fastctx_types::access::{Acting, Written};
use fastctx_types::on::OnOrigin;
use fastctx_types::remote::{RemoteCall, RemoteReply};
use fastctx_types::run::{CompletedJob, ConsumedEnd, JobDelivery};
use fastctx_types::residents::{Delivery, Notice};
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
}

#[derive(Clone, Serialize, Deserialize)]
struct Child {
    completed: CompletedJob,
    destination: Written,
    acting: Acting,
    origin: Option<OnOrigin>,
    copied: bool,
}

/// Durable before an inbox entry or the parent job can disappear. A remote
/// node receives the original host time, even after an offline interval.
pub(crate) fn confirmed(node: &Node, owner: Address, machine: MachineName, delivery: JobDelivery, acting: Option<&Acting>) -> io::Result<()> {
    if machine == node.machine() {
        received(node, &owner, &delivery)?;
    } else {
        let dispatched = crate::here::get_dispatched(node, &owner.resident, &delivery.job);
        let hub = dispatched.as_ref().and_then(|(destination, _)| destination.hub.clone());
        let acting = acting.cloned().unwrap_or_else(|| {
            node.residents.about(&owner.resident).map(|(about, _)| about.acting).unwrap_or_default()
        });
        let _gate = node.disk.lock().expect("never poisoned");
        let key = (owner.resident.clone(), delivery.job.clone());
        enqueue(node, Sending { owner, machine, acting, delivery: Some(delivery), consumption: None, origin: None, hub })?;
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
    let key = crate::speech::fixed_id("job-delivery", &[
        sending.owner.machine.as_str(), sending.owner.resident.as_str(), sending.machine.as_str(),
        job, output, request, phase,
    ]);
    let directory = node.world.path().join("job-deliveries");
    ensure_private_dir(&directory)?;
    queue(&directory.join(format!("{key}.json")), sending)
}

fn queue(path: &Path, sending: Sending) -> io::Result<()> {
    if let Some(mut prior) = fastctx_install::renewal::read_json::<Sending>(path)? {
        if prior.owner != sending.owner || prior.machine != sending.machine || prior.origin != sending.origin
            || (prior.hub.is_some() && sending.hub.is_some() && prior.hub != sending.hub)
        {
            return Err(io::Error::other("job delivery queue identity changed"));
        }
        if prior.hub.is_none() { prior.hub.clone_from(&sending.hub); }
        match (&mut prior.delivery, &sending.delivery, &prior.consumption, &sending.consumption) {
            (Some(prior), Some(next), None, None) if prior.job == next.job && prior.output == next.output => prior.shown_whole |= next.shown_whole,
            (None, None, Some(prior), Some(next)) if prior.id == next.id && prior.notice == next.notice => {},
            _ => return Err(io::Error::other("job delivery queue identity changed")),
        }
        fastctx_install::renewal::write_json(path, &prior)
    } else {
        create(path, &sending)
    }
}

fn take_sent(path: &Path, sent: &[u8]) -> io::Result<bool> {
    match fs::read(path) {
        Ok(current) if current == sent => { fs::remove_file(path)?; Ok(true) }
        Ok(_) => Ok(false),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(true),
        Err(error) => Err(error),
    }
}

pub(crate) fn received(node: &Node, owner: &Address, delivery: &JobDelivery) -> io::Result<()> {
    received_from(node, owner, delivery, None)
}

pub(crate) fn received_from(node: &Node, owner: &Address, delivery: &JobDelivery, origin: Option<&OnOrigin>) -> io::Result<()> {
    let _gate = node.disk.lock().expect("never poisoned");
    received_here(node, owner, delivery, origin, &mut HashSet::new())
}

fn received_here(node: &Node, owner: &Address, delivery: &JobDelivery, origin: Option<&OnOrigin>, seen: &mut HashSet<PathBuf>) -> io::Result<()> {
    let paths = if let Some(origin) = origin {
        vec![node.world.job_owner(&node.machine(), origin.town.as_ref(), &owner.clone().into()).job_record(&delivery.job)]
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
        if job.owner != *owner || job.id != delivery.job
            || (!delivery.output.is_empty() && job.output != delivery.output)
            || (delivery.output.is_empty() && origin.is_none())
            || origin.is_some_and(|origin| job.on.as_ref().and_then(|on| on.request.as_ref()) != origin.request.as_ref())
        { continue; }
        if let Some(origin) = origin
            && job.on.as_ref().and_then(|on| on.account.as_ref()) != origin.account.as_ref()
        { return Err(io::Error::other("the end receipt is for another account")); }
        if !seen.insert(path.clone()) { return Ok(()); }
        if job.end.is_none() { return Err(io::Error::other("a running job cannot have a delivered end")); }
        let marker = path.with_extension("delivered");
        let first = fastctx_install::renewal::read_json::<JobDelivery>(&marker)?;
        let mut confirmed = first.clone().unwrap_or_else(|| delivery.clone());
        confirmed.output.clone_from(&job.output);
        confirmed.shown_whole |= delivery.shown_whole;
        if let Some(children) = fastctx_install::renewal::read_json::<Vec<Child>>(&path.with_extension("waited"))? {
            for child in children {
                let receipt = JobDelivery { job: child.completed.job, output: child.completed.output, at: confirmed.at, shown_whole: child.copied };
                if node.is_here(&child.destination) {
                    received_here(node, owner, &receipt, child.origin.as_ref(), seen)?;
                } else {
                    enqueue(node, Sending { owner: owner.clone(), machine: child.destination.name, acting: child.acting,
                        delivery: Some(receipt), consumption: None, origin: child.origin, hub: child.destination.hub })?;
                }
            }
        }
        if first.as_ref().is_some_and(|prior| prior.shown_whole != confirmed.shown_whole) {
            fastctx_install::renewal::write_json(&marker, &confirmed)?;
        } else { create(&marker, &confirmed)?; }
        if confirmed.shown_whole { node.disk_due.lock().expect("never poisoned").insert(path.clone()); }
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
    if !children.iter().any(|prior| prior.completed == child.completed && prior.origin == child.origin) {
        children.push(child);
        fastctx_install::renewal::write_json(&marker, &children)?;
    }
    Ok(())
}

/// The held records are the ends actually written before TAKEN. Another
/// resident's wait neither consumes the owner's notice nor starts its clock.
pub(crate) fn wait_taken(node: &Node, caller: Option<&Address>, parent: Option<&JobId>, jobs: &[CompletedJob]) -> io::Result<()> {
    let Some(caller) = caller else { return Ok(()) };
    let _gate = node.disk.lock().expect("never poisoned");
    let parent = parent.map(|parent| {
        let path = crate::stop::find(&node.world, parent)?.ok_or_else(|| io::Error::other("the enclosing wait job disappeared"))?;
        let record = record::read(&path)?;
        if record.owner != *caller { return Err(io::Error::other("wait does not belong to the enclosing job")); }
        Ok(path)
    }).transpose()?;
    for completed in jobs {
        let path = crate::stop::find(&node.world, &completed.job)?.ok_or_else(|| io::Error::other("waited job disappeared"))?;
        let record = record::read(&path)?;
        if record.machine != completed.machine || record.output != completed.output || record.end.is_none() {
            return Err(io::Error::other("wait receipt does not match the ended job"));
        }
        if record.owner != *caller { continue; }
        if let Some(parent) = &parent {
            bind(parent, Child { completed: completed.clone(), destination: (&completed.machine).into(), acting: Acting::default(), origin: None, copied: false })?;
        } else {
            received_here(node, caller, &JobDelivery { job: completed.job.clone(), output: completed.output.clone(), at: Timestamp::now(), shown_whole: false }, None, &mut HashSet::new())?;
        }
    }
    Ok(())
}

/// Called only after the on CLI acknowledged a complete end. Its parent
/// will deliver this proof transitively when the final host receives it.
pub(crate) fn on_taken(node: &Node, parent: Option<&Path>, origin: &OnOrigin, destination: &Written, acting: &Acting, job: &JobId, consumed: &[ConsumedEnd]) -> io::Result<()> {
    let _gate = node.disk.lock().expect("never poisoned");
    let child = Child { completed: CompletedJob { machine: destination.name.clone(), job: job.clone(), output: String::new() },
        destination: destination.clone(), acting: acting.clone(), origin: Some(origin.clone()), copied: true };
    if let Some(parent) = parent {
        if record::read(parent)?.owner != origin.resident { return Err(io::Error::other("the on belongs to another enclosing job")); }
        bind(parent, child)?;
        let file = parent.with_extension("consumed");
        let mut prior = fastctx_install::renewal::read_json::<Vec<ConsumedEnd>>(&file)?.unwrap_or_default();
        for receipt in consumed {
            if !prior.contains(receipt) { prior.push(receipt.clone()); }
        }
        if !prior.is_empty() { fastctx_install::renewal::write_json(&file, &prior)?; }
    } else {
        let receipt = JobDelivery { job: job.clone(), output: String::new(), at: Timestamp::now(), shown_whole: true };
        if node.is_here(destination) {
            received_here(node, &origin.resident, &receipt, Some(origin), &mut HashSet::new())?;
        } else {
            enqueue(node, Sending { owner: origin.resident.clone(), machine: destination.name.clone(), acting: acting.clone(),
                delivery: Some(receipt), consumption: None, origin: Some(origin.clone()), hub: destination.hub.clone() })?;
        }
        node.disk_changed.notify_one();
    }
    Ok(())
}

pub(crate) fn apply_consumed(directory: &fastctx_types::layout::ResidentDir, consumed: &[ConsumedEnd]) -> io::Result<()> {
    for receipt in consumed {
        crate::inbox::consume(directory, &receipt.id)?;
        let letter = crate::speech::fixed_id("letter", &[receipt.machine.as_str(), receipt.id.as_str()]);
        crate::inbox::consume(directory, &letter)?;
    }
    Ok(())
}

/// Only this enclosing job's causal children enter its response. Historical
/// wait receipts are not an ever-growing payload of every later run.
pub(crate) fn snapshot(node: &Node, path: &Path) -> io::Result<Vec<ConsumedEnd>> {
    let _gate = node.disk.lock().expect("never poisoned");
    let mut receipts = Vec::new();
    snapshot_here(node, path, &mut HashSet::new(), &mut receipts)?;
    Ok(receipts)
}

fn snapshot_here(node: &Node, path: &Path, seen: &mut HashSet<PathBuf>, receipts: &mut Vec<ConsumedEnd>) -> io::Result<()> {
    if !seen.insert(path.to_path_buf()) { return Ok(()); }
    let record = match record::read(path) {
        Ok(record) => record,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if let Some(consumed) = fastctx_install::renewal::read_json::<Vec<ConsumedEnd>>(&path.with_extension("consumed"))? {
        for receipt in consumed { if !receipts.contains(&receipt) { receipts.push(receipt); } }
    }
    if let Some(children) = fastctx_install::renewal::read_json::<Vec<Child>>(&path.with_extension("waited"))? {
        for child in children {
            if !node.is_here(&child.destination) { continue; }
            for parent in node.world.job_parents()? {
                for directory in crate::retention::entries(&parent)? {
                    let path = directory.join("jobs").join(format!("{}.json", child.completed.job));
                    let target = match record::read(&path) {
                        Ok(record) => record,
                        Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                        Err(error) => return Err(error),
                    };
                    if target.owner != record.owner || (!child.completed.output.is_empty() && target.output != child.completed.output)
                        || child.origin.as_ref().is_some_and(|origin| target.on.as_ref().and_then(|on| on.request.as_ref()) != origin.request.as_ref())
                    { continue; }
                    let marker = path.with_extension("wait-taken");
                    match fs::read_to_string(&marker) {
                        Ok(id) => {
                            let id = if id.is_empty() { crate::inbox::job_notice_id(&target) } else { id.parse().map_err(io::Error::other)? };
                            let receipt = ConsumedEnd { machine: target.machine.clone(), id };
                            if !receipts.contains(&receipt) { receipts.push(receipt); }
                        },
                        Err(error) if error.kind() == io::ErrorKind::NotFound => {},
                        Err(error) => return Err(error),
                    }
                    snapshot_here(node, &path, seen, receipts)?;
                }
            }
        }
    }
    Ok(())
}

pub(crate) async fn supports(node: &Node, acting: &Acting, destination: &Written) -> Result<bool, String> {
    match node.call(acting, destination, fastctx_types::access::Purpose::Look, RemoteCall::Facts).await {
        Ok(Ok(RemoteReply::Facts(facts))) => Ok(facts.protocol.is_some_and(|protocol| protocol >= 11)),
        other => Err(format!("checking {}'s end-receipt support: {other:?}", destination.name)),
    }
}

/// A consumed end is durable even while its owner is offline. This phase
/// suppresses notification; it contains no retention timestamp.
pub(crate) fn consumed(node: &Node, owner: &Address, acting: &Acting, id: NoticeId, notice: Notice) -> io::Result<()> {
    let _gate = node.disk.lock().expect("never poisoned");
    enqueue(node, Sending { owner: owner.clone(), machine: owner.machine.clone(), acting: acting.clone(),
        delivery: None, consumption: Some(Delivery { id, at: Timestamp::now(), notice }), origin: None, hub: None })?;
    node.disk_changed.notify_one();
    Ok(())
}

pub(crate) async fn forward(node: &std::sync::Arc<Node>) -> io::Result<()> {
    let mut capabilities = Vec::<(Written, Acting, bool)>::new();
    for path in crate::retention::entries(&node.world.path().join("job-deliveries"))? {
        if path.extension().is_none_or(|extension| extension != "json") { continue; }
        let bytes = fs::read(&path)?;
        let sending: Sending = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
        let destination = Written { name: sending.machine.clone(), hub: sending.hub.clone() };
        let capable = match capabilities.iter().find(|(target, acting, _)| *target == destination && *acting == sending.acting) {
            Some((_, _, capable)) => *capable,
            None => {
                let capable = match supports(node, &sending.acting, &destination).await {
                    Ok(capable) => capable,
                    Err(error) => { node.log.line(format_args!("job receipt remains pending: {error}")); continue; },
                };
                capabilities.push((destination.clone(), sending.acting.clone(), capable));
                capable
            }
        };
        if !capable { continue; }
        let sent = if let Some(delivery) = sending.delivery {
            let call = RemoteCall::Forget { resident: sending.owner.resident, town: sending.origin.as_ref().and_then(|origin| origin.town.clone()),
                job: delivery.job.clone(), delivery: Some(delivery), origin: sending.origin };
            match node.call(&sending.acting, &destination, fastctx_types::access::Purpose::Hands, call).await {
                Ok(Ok(RemoteReply::Set)) => true,
                result => { node.log.line(format_args!("job delivery to {} remains pending: {result:?}", sending.machine)); false },
            }
        } else if let Some(consumption) = sending.consumption {
            match crate::speech::tell(node, &sending.acting, &sending.owner, consumption.id, consumption.notice, crate::speech::Owed::Working).await {
                Ok(_) => true,
                Err(error) => { node.log.line(format_args!("job consumption to {} remains pending: {error}", sending.machine)); false },
            }
        } else { return Err(io::Error::other("invalid completion receipt phase")); };
        if sent {
                let _gate = node.disk.lock().expect("never poisoned");
                if !take_sent(&path, &bytes)? { node.disk_changed.notify_one(); }
        }
    }
    Ok(())
}

fn create(path: &Path, value: &impl Serialize) -> io::Result<()> {
    match publish_new(path, &serde_json::to_vec(value).map_err(io::Error::other)?, Permissions::Private) {
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        result => result,
    }
}
