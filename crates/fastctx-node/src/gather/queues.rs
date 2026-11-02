//! Old shared queues become conversation-owned without changing path-bound
//! request identities. The origin map is durable before the first file moves.
use super::*;
use fastctx_types::{JobId, access::HubAccount, run::JobRecord};

pub(super) fn gather(world: &WorldDir, failed: &BTreeSet<String>, gathered: &mut Gathered) -> io::Result<()> {
    let own = crate::machine::names(world)?;
    let here = fastctx_types::layout::Names { me: own.name.clone(), was: own.was.clone(), renamed: Vec::new() };
    let mut dispatches: Option<BTreeMap<String, Vec<JobRecord>>> = None;
    for name in
        ["received", "received-controls", "job-deliveries", "on-deliveries", "on-stops", "run-stops", "run-requests"]
    {
        let shared = world.path().join(name);
        for path in entries(&shared)? {
            if path.extension().is_none_or(|extension| extension != "json") {
                continue;
            }
            let result = (|| -> io::Result<()> {
                let value: serde_json::Value = read_json(&path)?;
                let owner: Address = match name {
                    "received" | "on-deliveries" => {
                        Address { machine: own.name.clone(), resident: serde_json::from_value(value["to"].clone())? }
                    }
                    "received-controls" => serde_json::from_value::<fastctx_types::delivery_control::ControlLetter>(
                        value["command"].clone(),
                    )?
                    .owner()
                    .clone(),
                    "on-stops" => serde_json::from_value(value["origin"]["resident"].clone())?,
                    "run-requests" => {
                        if dispatches.is_none() {
                            dispatches = Some(job_dispatches(world)?);
                        }
                        let job: JobId = serde_json::from_value(value["job"].clone())?;
                        let scope: HubAccount = serde_json::from_value(value["scope"].clone())?;
                        let key = path
                            .file_stem()
                            .and_then(|name| name.to_str())
                            .ok_or_else(|| io::Error::other("invalid dispatch filename"))?;
                        let matches: Vec<_> = dispatches
                            .as_ref()
                            .expect("loaded dispatches")
                            .get(key)
                            .into_iter()
                            .flatten()
                            .filter(|record| record.id == job && record.account.as_ref() == Some(&scope))
                            .collect();
                        let [record] = matches.as_slice() else {
                            return Err(io::Error::other("dispatch has no unique matching job incarnation; retained"));
                        };
                        record
                            .owner
                            .resident()
                            .cloned()
                            .ok_or_else(|| io::Error::other("dispatch job has no conversation owner"))?
                    }
                    _ => serde_json::from_value(value["owner"].clone())?,
                };
                let room = world.owner(&here, &owner);
                let owner_name = room.path().file_name().and_then(|name| name.to_str()).expect("conversation name");
                if failed.contains(owner_name) {
                    return Ok(());
                }
                let directory = room.book(name);
                let target = directory.join(path.file_name().expect("queue record"));
                // Reject the whole move before publishing a route or mixing sidecars.
                for extension in ["json", "taken", "stopped", "retry", "done", "posted", "reply", "answer"] {
                    let source = path.with_extension(extension);
                    let destination = target.with_extension(extension);
                    if fastctx_platform::disk::is_there(&source)
                        && fastctx_platform::disk::is_there(&destination)
                        && fastctx_platform::disk::read(&source)? != fastctx_platform::disk::read(&destination)?
                    {
                        retain_conflict(&room, name, &path)?;
                        return Err(io::Error::other("conflicting queue records retained for isolated archival"));
                    }
                }
                let origin_file = room.path().join(records::ORIGINS);
                let mut origins: BTreeMap<PathBuf, PathBuf> = read_optional(&origin_file)?.unwrap_or_default();
                let relative = target.strip_prefix(room.path()).expect("queue in room").with_extension("");
                let rejected: BTreeSet<PathBuf> =
                    read_optional(&room.path().join(records::REJECTIONS))?.unwrap_or_default();
                if rejected.contains(&relative) {
                    retain_conflict(&room, name, &path)?;
                    return Err(io::Error::other("rejected legacy queue identity retained for isolated archival"));
                }
                let original = path.with_extension("");
                if origins.get(&relative).is_some_and(|prior| prior != &original) {
                    return Err(io::Error::other("conflicting queue origin"));
                }
                origins.insert(relative, original);
                records::moving(&target)?;
                publish(&origin_file, &json(&origins)?, Permissions::Private)?;
                for extension in ["taken", "stopped", "retry", "done", "posted", "reply", "answer"] {
                    let auxiliary = path.with_extension(extension);
                    if fastctx_platform::disk::is_there(&auxiliary) {
                        move_one(&auxiliary, &target.with_extension(extension))?;
                        remove_file(&auxiliary)?;
                    }
                }
                move_one(&path, &target)?;
                remove_file(&path)
            })();
            if let Err(error) = result {
                gathered.problems.push((path, error));
            }
        }
        if entries(&shared)?.is_empty() {
            remove_dir(&shared)?;
        }
    }
    Ok(())
}
fn job_dispatches(world: &WorldDir) -> io::Result<BTreeMap<String, Vec<JobRecord>>> {
    let mut records: BTreeMap<String, Vec<JobRecord>> = BTreeMap::new();
    let mut seen = BTreeSet::new();
    for parent in world.job_parents()? {
        for owner in entries(&parent)? {
            for path in entries(&owner.join("jobs"))? {
                if path.extension().is_none_or(|extension| extension != "json") {
                    continue;
                }
                let canonical = fastctx_platform::disk::canonicalize(&path)?;
                if !seen.insert(canonical) {
                    continue;
                }
                let dispatch = match fastctx_platform::disk::read_to_string(path.with_extension("run-request")) {
                    Ok(dispatch) => dispatch,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                    Err(error) => return Err(error),
                };
                if fastctx_types::NoticeId::try_from(dispatch.as_str()).is_err() {
                    continue;
                }
                if let Ok(record) = fastctx_hands::run::record::read(&path) {
                    records.entry(dispatch).or_default().push(record);
                }
            }
        }
    }
    Ok(records)
}

fn retain_conflict(room: &ConversationDir, queue: &str, source: &Path) -> io::Result<()> {
    let signature = crate::record_migration::archive::fingerprint(source)?;
    let recovered = room.path().join("recovered/queues").join(queue).join(signature.digest);
    // The envelope moves last, so a restart still finds an unfinished group.
    for extension in ["taken", "stopped", "retry", "done", "posted", "reply", "answer", "json"] {
        let path = source.with_extension(extension);
        if fastctx_platform::disk::is_there(&path) {
            move_one(&path, &recovered.join(path.file_name().expect("queue filename")))?;
            remove_file(&path)?;
        }
    }
    Ok(())
}

pub(crate) fn prepare_conflicts(room: &ConversationDir) -> io::Result<()> {
    let root = room.path().join("recovered/queues");
    crate::records::ensure_each(room, "gathered-queue-conflicts", move |push| {
        for queue in entries(&root)? {
            for group in entries(&queue)? {
                for path in entries(&group)? {
                    crate::record_migration::file(&path, push, |_| {
                        Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "legacy queue envelope or sibling conflicts with the existing conversation queue",
                        ))
                    })?;
                }
            }
        }
        Ok(())
    })
}
