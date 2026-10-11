//! Command artifacts expire after their end reaches its reader. Product
//! records and unconfirmed delivery evidence never enter this collector.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use fastctx_hands::run::{program, record};
use fastctx_platform::{Permissions, publish_new};
use fastctx_types::run::JobDelivery;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::Node;

const DAY: u64 = 24 * 60 * 60;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Settings {
    command_artifact_days: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Self { command_artifact_days: 7 }
    }
}

pub(crate) use crate::job_receipts::{confirmed, received, received_from, wait_taken};

pub(crate) fn start(node: &Arc<Node>) -> io::Result<()> {
    use notify::Watcher as _;
    let settings = node.world.path().join("retention.json");
    create(&settings, &Settings::default())?;
    let weak = Arc::downgrade(node);
    let settings_changed = Arc::new(tokio::sync::Notify::new());
    let changed = settings_changed.clone();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        if let Some(node) = weak.upgrade() {
            match event {
                Ok(event)
                    if event.paths.iter().any(|path| path.file_name().is_some_and(|name| name == "retention.json")) =>
                {
                    changed.notify_one()
                }
                Ok(_) => {}
                Err(error) => node.log.line(format_args!("watching retention settings: {error}")),
            }
        }
    })
    .map_err(io::Error::other)?;
    watcher.watch(node.world.path(), notify::RecursiveMode::NonRecursive).map_err(io::Error::other)?;
    let node = node.clone();
    tokio::spawn(async move {
        let _watcher = watcher;
        // Re-read settings and retry idle program collection once per hour:
        // 24 observations per retention day, without a deadline for work.
        let mut schedule = tokio::time::interval(Duration::from_secs(DAY / 24));
        loop {
            let maintenance = tokio::select! {
                _ = schedule.tick() => true,
                () = settings_changed.notified() => true,
                () = node.disk_changed.notified() => false,
            };
            if let Err(error) = crate::job_receipts::forward(&node).await {
                node.log.line(format_args!("forwarding job delivery receipts: {error}"));
            }
            let collecting = node.clone();
            match tokio::task::spawn_blocking(move || {
                let immediate = collect_immediate(&collecting);
                let retained = if maintenance { collect(&collecting, Timestamp::now()) } else { Ok(()) };
                immediate.and(retained)
            })
            .await
            {
                Ok(Ok(())) => {}
                Ok(Err(error)) => node.log.line(format_args!("collecting command artifacts: {error}")),
                Err(error) => node.log.line(format_args!("collecting command artifacts: {error}")),
            }
            if !maintenance {
                continue;
            }
            let collecting = node.clone();
            match tokio::task::spawn_blocking(move || crate::program_collection::collect(&collecting)).await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => node.log.line(format_args!("collecting unused programs: {error:#}")),
                Err(error) => node.log.line(format_args!("collecting unused programs: {error}")),
            }
        }
    });
    Ok(())
}

fn collect_immediate(node: &Arc<Node>) -> io::Result<()> {
    let _gate = node.disk.lock().expect("never poisoned");
    let paths: Vec<_> = node.disk_due.lock().expect("never poisoned").drain().collect();
    let mut failures = Vec::new();
    for path in paths {
        match record::read(&path) {
            Ok(job) if program::in_use(&job)? => {
                node.disk_due.lock().expect("never poisoned").insert(path);
                continue;
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => {
                failures.push(format!("{}: {error}", path.display()));
                node.disk_due.lock().expect("never poisoned").insert(path);
                continue;
            }
        }
        if node.job_readers.lock().expect("never poisoned").contains_key(&path) {
            node.disk_due.lock().expect("never poisoned").insert(path);
            continue;
        }
        let collected = if path.with_extension("delivered").try_exists()? {
            crate::job_watch::collect(node, &path)
        } else if path.with_extension("copied").try_exists()? {
            crate::jobs::copy_output(&path)
        } else { Ok(()) };
        match collected {
            Ok(()) => {}
            Err(error) => {
                failures.push(format!("{}: {error}", path.display()));
                node.disk_due.lock().expect("never poisoned").insert(path);
            }
        }
    }
    if failures.is_empty() { Ok(()) } else { Err(io::Error::other(failures.join("\n"))) }
}

fn collect(node: &Arc<Node>, now: Timestamp) -> io::Result<()> {
    let settings: Settings =
        serde_json::from_slice(&fs::read(node.world.path().join("retention.json"))?).map_err(io::Error::other)?;
    let lifetime = i64::from(settings.command_artifact_days) * DAY as i64;
    let _collecting = node.disk.lock().expect("never poisoned");
    collect_records(
        &node.world,
        now,
        lifetime,
        |path| {
            crate::on::unfollowed(node, path) && !node.job_readers.lock().expect("never poisoned").contains_key(path)
        },
        |path| crate::job_watch::collect(node, path),
        |directory| match fastctx_install::renewal::read_json::<fastctx_types::residents::Now>(&directory.join("now")) {
            Ok(now) => now.is_none_or(|now| now.activity != fastctx_types::residents::Activity::Busy),
            Err(error) => {
                node.log.line(format_args!("reading temporary artifacts' owner: {error}"));
                false
            }
        },
    )
}

fn collect_records(
    world: &fastctx_types::layout::WorldDir,
    now: Timestamp,
    lifetime: i64,
    unfollowed: impl Fn(&Path) -> bool,
    forget: impl Fn(&Path) -> io::Result<()>,
    idle: impl Fn(&Path) -> bool,
) -> io::Result<()> {
    for parent in world.job_parents()? {
        for directory in entries(&parent)? {
            if fastctx_platform::owned_directory::is_link(&fs::symlink_metadata(&directory)?) {
                continue;
            }
            let latest = fastctx_install::renewal::read_json::<Timestamp>(&directory.join("artifacts-delivered.json"))?;
            let mut pending = false;
            for path in entries(&directory.join("jobs"))? {
                if path.extension().is_none_or(|extension| extension != "json") {
                    continue;
                }
                let job = record::read(&path)?;
                let marker = path.with_extension("delivered");
                let delivery = match fs::read(&marker) {
                    Ok(bytes) => serde_json::from_slice::<JobDelivery>(&bytes).map_err(io::Error::other)?,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {
                        if path.with_extension("copied").try_exists()? && !program::in_use(&job)? && unfollowed(&path) {
                            crate::jobs::copy_output(&path)?;
                        }
                        let Some(delivery) = legacy_delivery(world, &path, &job)? else {
                            pending = true;
                            continue;
                        };
                        create(&marker, &delivery)?;
                        let latest_path = directory.join("artifacts-delivered.json");
                        let prior = fastctx_install::renewal::read_json::<Timestamp>(&latest_path)?;
                        if prior.is_none_or(|prior| prior < delivery.at) {
                            fastctx_install::renewal::write_json(&latest_path, &delivery.at)?;
                        }
                        delivery
                    }
                    Err(error) => return Err(error),
                };
                if job.end.is_none()
                    || program::in_use(&job)?
                    || !unfollowed(&path)
                    || path.with_extension("owed").try_exists()?
                {
                    pending = true;
                    continue;
                }
                if delivery.shown_whole || expired(now, delivery.at, lifetime) {
                    // Watches still read the record after the supervisor exits.
                    // Their own forgetting mechanism serializes that last reader.
                    forget(&path)?;
                } else {
                    pending = true;
                }
            }
            for scratch in entries(&directory.join("out"))? {
                let Some(job) = scratch
                    .file_name()
                    .and_then(|name| name.to_str())
                    .and_then(|name| name.strip_suffix(".tmp"))
                    .and_then(|name| name.parse::<fastctx_types::JobId>().ok())
                else {
                    continue;
                };
                if !scratch.is_dir()
                    || fastctx_platform::owned_directory::is_link(&fs::symlink_metadata(&scratch)?)
                    || directory.join("jobs").join(format!("{job}.json")).try_exists()?
                {
                    continue;
                }
                if let Some(delivered) = fastctx_install::renewal::read_json::<Timestamp>(&scratch.join(".delivered"))?
                    && expired(now, delivered, lifetime)
                {
                    fastctx_platform::owned_directory::delete(&scratch)?;
                }
            }
            if !pending
                && idle(&directory)
                && let Some(delivered) = latest
                && expired(now, delivered, lifetime)
            {
                collect_out(&directory.join("out"), delivered)?;
            }
        }
    }
    Ok(())
}

fn legacy_delivery(
    world: &fastctx_types::layout::WorldDir,
    path: &Path,
    job: &fastctx_types::run::JobRecord,
) -> io::Result<Option<JobDelivery>> {
    let Some(owner) = job.owner.resident() else { return Ok(None) };
    if job.on.is_some() || path.with_extension("wait-taken").try_exists()? || path.with_extension("copied").try_exists()? {
        return Ok(None);
    }
    // Only a local resident's own execution can share this machine's
    // consuming-turn proof. A queued remote letter is not host delivery.
    if owner.machine != job.machine
        || path
            .parent()
            .and_then(Path::parent)
            .and_then(Path::file_name)
            .is_none_or(|name| name != owner.resident.as_str())
    {
        return Ok(None);
    }
    let Some(notice) =
        fastctx_install::renewal::read_json::<fastctx_types::residents::Notice>(&path.with_extension("notice"))?
    else {
        return Ok(None);
    };
    if !matches!(notice, fastctx_types::residents::Notice::JobEnded { job: ref noticed, ref machine, ref output, .. } if noticed == &job.id && machine == &job.machine && output == &job.output)
    {
        return Ok(None);
    }
    let id = match job.on.as_ref().and_then(|on| on.request.as_ref()) {
        Some(request) => crate::speech::fixed_id(
            "on-end",
            &[job.machine.as_str(), owner.machine.as_str(), owner.resident.as_str(), job.id.as_str(), request.as_str()],
        ),
        None => format!("{}-{}", job.machine, job.id).parse().map_err(io::Error::other)?,
    };
    for parent in world.resident_parents()? {
        let resident = parent.join(owner.resident.as_str());
        for directory in [resident.clone(), resident.join("archive")] {
            let receipt = directory.join("delivery-receipts").join(id.as_str());
            let metadata = match fs::symlink_metadata(&receipt) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            };
            if !metadata.is_file() || fastctx_platform::owned_directory::is_link(&metadata) {
                return Err(io::Error::other("unexpected legacy delivery receipt"));
            }
            if !fs::read(&receipt)?.is_empty() { continue; }
            let at = Timestamp::try_from(metadata.modified()?).map_err(io::Error::other)?;
            return Ok(Some(JobDelivery { job: job.id.clone(), output: job.output.clone(), at, shown_whole: false }));
        }
    }
    Ok(None)
}

fn expired(now: Timestamp, delivered: Timestamp, lifetime: i64) -> bool {
    now.as_nanosecond() - delivered.as_nanosecond() >= i128::from(lifetime) * 1_000_000_000
}

fn collect_out(path: &Path, delivered: Timestamp) -> io::Result<()> {
    for path in entries(path)? {
        let metadata = fs::symlink_metadata(&path)?;
        let modified = Timestamp::try_from(metadata.modified()?).map_err(io::Error::other)?;
        if modified > delivered {
            continue;
        }
        if fastctx_platform::owned_directory::is_link(&metadata) {
            continue;
        }
        if metadata.is_dir() {
            collect_out(&path, delivered)?;
            if entries(&path)?.is_empty() {
                fs::remove_dir(path)?;
            }
        } else {
            fs::remove_file(path)?;
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

pub(crate) fn entries(path: &Path) -> io::Result<Vec<PathBuf>> {
    match fs::read_dir(path) {
        Ok(entries) => entries.map(|entry| entry.map(|entry| entry.path())).collect(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error),
    }
}
