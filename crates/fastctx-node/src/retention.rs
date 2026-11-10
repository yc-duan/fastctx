//! Runtime maintenance keeps delivered job assets until explicit deletion.

use fastctx_platform::disk as fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use fastctx_hands::run::{program, record};

use crate::Node;

pub(crate) use crate::job_receipts::{confirmed, received, received_from, wait_taken};

/// The hourly maintenance is a pass (`crate::passes`): made as the node
/// starts and an hour after each one ends.
const MAINTAINING: crate::passes::Pass = crate::passes::Pass {
    name: "upkeep/hourly",
    within: Duration::from_secs(50 * 60),
    rests: Some(Duration::from_secs(60 * 60)),
};

pub(crate) fn start(node: &Arc<Node>) -> io::Result<()> {
    crate::claude::claim_cleanup::start(node);
    crate::passes::register(node, MAINTAINING, |node: Arc<Node>| async move {
        maintain(&node).await;
        crate::passes::Left::Nothing
    });
    // What a finished job leaves is collected as it finishes, between passes.
    let node = node.clone();
    fastctx_platform::task::spawn(async move {
        loop {
            node.job_files.changed.notified().await;
            collect(&node, false).await;
        }
    });
    Ok(())
}

/// Retires what completed jobs left; with `maintenance`, what only the
/// hourly pass looks at too.
async fn collect(node: &Arc<Node>, maintenance: bool) {
    let collecting = node.clone();
    match fastctx_platform::task::spawn_blocking(move || {
        recover_book(&collecting);
        let immediate = collect_immediate(&collecting);
        let retained = if maintenance {
            let obligations = crate::obligations::collect(&collecting);
            let retained = collect_scratch(&collecting);
            obligations.and(retained)
        } else {
            Ok(())
        };
        immediate.and(retained)
    })
    .await
    {
        Ok(Ok(())) => {}
        Ok(Err(error)) => node.log.line(format_args!("retiring completed jobs: {error}")),
        Err(error) => node.log.line(format_args!("retiring completed jobs: {error}")),
    }
}

/// One hourly pass: every step is told on its own when it fails, and none
/// holds back the others.
async fn maintain(node: &Arc<Node>) {
    collect(node, true).await;
    let retiring = node.clone();
    match fastctx_platform::task::spawn_blocking(move || {
        let payloads = crate::record_payloads::maintain(&retiring.database, |scope, error| {
            retiring.log.line(format_args!("collecting conversation {scope} projections: {error}"))
        })
        .map_err(io::Error::other);
        let maps = crate::records::maps::maintain(&retiring.database, |scope, error| {
            retiring.log.line(format_args!("collecting conversation {scope} map snapshots: {error}"))
        })
        .map_err(io::Error::other);
        let requests = crate::claude::turn_facts::collect(&retiring.database, |scope, error| {
            retiring.log.line(format_args!("collecting conversation {scope} request observations: {error}"))
        })
        .map_err(io::Error::other);
        let terminals = crate::delivery::terminals::collect(&retiring.database, |scope, error| {
            retiring.log.line(format_args!("collecting conversation {scope} staged terminal readers: {error}"))
        })
        .map_err(io::Error::other);
        let books = crate::books::retire(&retiring);
        let presence = crate::inbox::presence::collect(&retiring.database);
        let claude = crate::claude::garbage::collect(&retiring.world);
        let trust = crate::claude::garbage::untrust_departed(&retiring);
        let purged = crate::activity_book::purge(&retiring);
        let swept = crate::gather::sweep(&retiring.world);
        // Each step is told on its own when it fails.
        [
            ("collecting conversation projections", payloads),
            ("collecting conversation maps", maps),
            ("collecting staged terminal readers", terminals),
            ("collecting settled request observations", requests),
            ("collecting closed input memberships", presence),
            ("retiring the bookkeeping of removed conversations", books),
            ("collecting what Claude sessions left", claude),
            ("taking back Claude Code's trust of deleted conversations", trust),
            ("letting go of deleted conversations' activity", purged),
            ("sweeping earlier conversation directories", swept),
        ]
        .into_iter()
        .filter_map(|(step, result)| result.err().map(|error| format!("{step}: {error}")))
        .collect::<Vec<_>>()
    })
    .await
    {
        Ok(failures) => {
            for failure in failures {
                node.log.line(format_args!("{failure}"));
            }
        }
        Err(error) => node.log.line(format_args!("retiring bookkeeping: {error}")),
    }
    let collecting = node.clone();
    match fastctx_platform::task::spawn_blocking(move || crate::program_collection::collect(&collecting)).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => node.log.line(format_args!("collecting unused programs: {error:#}")),
        Err(error) => node.log.line(format_args!("collecting unused programs: {error}")),
    }
}

/// Recovery holds a job lease while replaying journals, without a shared lock.
/// Failed jobs stay eligible for the next maintenance round; one does not stop others.
fn recover_book(node: &Arc<Node>) {
    let paths: Vec<_> = node.job_files.due.lock().expect("never poisoned").iter().cloned().collect();
    for path in paths {
        let Some(lease) = crate::job_reader::hold(node, &path) else {
            continue;
        };
        let record = match record::read(&path) {
            Ok(record) if record.end.is_some() => record,
            _ => continue,
        };
        node.job_files.due.lock().expect("never poisoned").remove(&path);
        match crate::activity_book::consumed(node, &record) {
            Ok(true) => {}
            Ok(false) => {
                if let Err(error) = crate::activity_book::observe(node, &path) {
                    node.log.line(format_args!("recovering activity for job {}: {error}", record.id));
                }
            }
            Err(error) => node.log.line(format_args!("checking activity for job {}: {error}", record.id)),
        }
        drop(lease);
        node.job_files.due.lock().expect("never poisoned").insert(path);
    }
}

fn collect_immediate(node: &Arc<Node>) -> io::Result<()> {
    let paths: Vec<_> = node.job_files.due.lock().expect("never poisoned").drain().collect();
    let mut failures = Vec::new();
    for path in paths {
        match collect_due(node, &path) {
            Ok(true) => {}
            Ok(false) => {
                node.job_files.due.lock().expect("never poisoned").insert(path);
            }
            Err(error) => {
                failures.push(format!("{}: {error}", path.display()));
                node.job_files.due.lock().expect("never poisoned").insert(path);
            }
        }
    }
    if failures.is_empty() { Ok(()) } else { Err(io::Error::other(failures.join("\n"))) }
}

/// Checks one due job under its own gate, then atomically exchanges its lease
/// for collection. Other jobs never wait for these reads or deletions.
fn collect_due(node: &Arc<Node>, path: &Path) -> io::Result<bool> {
    let Some(files) = node.job_files.hold(path) else { return Ok(false) };
    let gate = node.job_files.gate(path);
    let _job = gate.lock();
    let job = match record::read(path) {
        Ok(job) => job,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(true),
        Err(error) => return Err(error),
    };
    if program::in_use(&job)? {
        return Ok(false);
    }
    if !crate::job_metadata::exists(&path.with_extension("delivered"))? {
        return Ok(true);
    }
    let Ok(_marked) = files.collect() else { return Ok(false) };
    crate::job_watch::collect(node, path)?;
    Ok(true)
}

/// Scratch remains temporary even when the completed job record is retained.
fn collect_scratch(node: &Arc<Node>) -> io::Result<()> {
    for parent in node.world.job_parents()? {
        for directory in entries(&parent)? {
            if fastctx_platform::owned_directory::is_link(&fs::symlink_metadata(&directory)?)
                || !retired_owner(&node.world, &directory)?
            {
                continue;
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
                let Ok(_marked) = node.job_files.collect(vec![scratch.clone()]) else { continue };
                if !fs::is_dir(&scratch)
                    || fastctx_platform::owned_directory::is_link(&fs::symlink_metadata(&scratch)?)
                    || !fs::exists(scratch.join(".delivered"))?
                {
                    continue;
                }
                let path = directory.join("jobs").join(format!("{job}.json"));
                match record::read(&path) {
                    Ok(job)
                        if job.end.is_none()
                            || program::in_use(&job)?
                            || !crate::on::unfollowed(node, &path)
                            || crate::job_metadata::exists(&path.with_extension("owed"))? =>
                    {
                        continue;
                    }
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error),
                    Ok(_) => {}
                }
                let now = fastctx_install::renewal::read_json::<fastctx_types::residents::Now>(&directory.join("now"))?;
                if !retired_owner(&node.world, &directory)?
                    || now.is_some_and(|now| now.activity == fastctx_types::residents::Activity::Busy)
                {
                    continue;
                }
                fastctx_platform::owned_directory::delete(&scratch)?;
            }
        }
    }
    Ok(())
}

/// Temporary paths remain usable for as long as their conversation does.
/// A removed home whose books retired has no model or delivery reader left.
fn retired_owner(world: &fastctx_types::layout::WorldDir, directory: &Path) -> io::Result<bool> {
    let directory = if directory.parent().and_then(Path::file_name).is_some_and(|name| name == "legacy") {
        directory.parent().and_then(Path::parent).expect("legacy is inside a conversation")
    } else {
        directory
    };
    let Some(name) = directory.file_name().and_then(|name| name.to_str()) else { return Ok(false) };
    let Some(fastctx_types::layout::Kind::Home(id)) = fastctx_types::layout::Kind::of(name) else { return Ok(false) };
    let home = world.conversation(&id);
    Ok(crate::residents::read_removed(&home)?.is_some()
        && fastctx_platform::disk::exists(home.path().join("books-retired"))?)
}

pub(crate) fn entries(path: &Path) -> io::Result<Vec<PathBuf>> {
    match fs::read_dir(path) {
        Ok(entries) => entries.map(|entry| entry.map(|entry| entry.path())).collect(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error),
    }
}
