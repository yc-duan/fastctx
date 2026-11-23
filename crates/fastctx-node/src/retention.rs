//! Runtime maintenance: what completed jobs leave is retired as they end,
//! and the files of a job go some days after its answer reached the model
//! (`expire`), once its output is kept for good where its conversation lives.

use fastctx_platform::disk as fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use fastctx_hands::run::{program, record};
use fastctx_types::run::JobDelivery;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::Node;

/// How many days a job's files stay after its answer reached the model, as
/// this machine is set (`days`), unless set otherwise.
const DEFAULT_DAYS: u32 = 7;
/// The most days a job run here for a conversation elsewhere waits for that
/// conversation's machine to take its output whole.
const UNCOLLECTED_DAYS: i64 = 30;
const DAY_MS: i64 = 24 * 60 * 60 * 1000;
/// The most jobs whose files go in one pass, so that a machine that kept
/// thousands lets them go over a few hours.
const EXPIRED_AT_ONCE: usize = 2000;

#[derive(Serialize, Deserialize)]
struct Settings {
    command_artifact_days: u32,
}

/// How many days a job's files stay after its answer reached the model.
pub(crate) fn days(world: &fastctx_types::layout::WorldDir) -> io::Result<u32> {
    match fs::read(world.retention()) {
        Ok(bytes) => serde_json::from_slice::<Settings>(&bytes)
            .map(|settings| settings.command_artifact_days)
            .map_err(io::Error::other),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(DEFAULT_DAYS),
        Err(error) => Err(error),
    }
}

/// Keeps a job's files `days` days after its answer reached the model.
pub(crate) fn set_days(world: &fastctx_types::layout::WorldDir, days: u32) -> io::Result<()> {
    let bytes = serde_json::to_vec(&Settings { command_artifact_days: days }).map_err(io::Error::other)?;
    fastctx_platform::publish(&world.retention(), &bytes, fastctx_platform::Permissions::Private)
}

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
            let guests = collect_guest_scratch(&collecting);
            let expired = expire(&collecting);
            obligations.and(retained).and(guests).and(expired)
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
    expire_guest_rooms(node).await;
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
    // Its output in the asset library whole and taken by the hub first: the
    // source goes after (plan 3-4 group 3).
    if !crate::job_assets::kept_for_good(node, &job) {
        return Ok(false);
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

/// How long a guest room's scratch stays after the last command its guest
/// ran here, with none still running. A guest room is where a conversation
/// of another machine, a colleague most often, worked here; its scratch is
/// temporary (`sync-data-classes`: scratch is not synced), and a colleague's
/// work is handed over long before this.
const GUEST_SCRATCH_KEPT: jiff::SignedDuration = jiff::SignedDuration::from_hours(14 * 24);

/// Empties the scratch of guest rooms whose guest has run nothing here for
/// `GUEST_SCRATCH_KEPT`. The room's records stay; the data page still lists
/// it and deletes it whole when asked (`crate::data`).
fn collect_guest_scratch(node: &Arc<Node>) -> io::Result<()> {
    let now = jiff::Timestamp::now();
    for directory in entries(&node.world.conversations())? {
        let Some(name) = directory.file_name().and_then(|name| name.to_str()) else { continue };
        if !matches!(
            fastctx_types::layout::Kind::of(name),
            Some(fastctx_types::layout::Kind::Guest(_) | fastctx_types::layout::Kind::Visiting { .. })
        ) {
            continue;
        }
        match empty_guest_scratch(node, &directory, now) {
            Ok(false) => {}
            Ok(true) => node.log.line(format_args!(
                "emptied the scratch of guest room {name}: its guest ran nothing here for {} days",
                GUEST_SCRATCH_KEPT.as_hours() / 24
            )),
            Err(error) => node.log.line(format_args!("emptying the scratch of guest room {name}: {error}")),
        }
    }
    Ok(())
}

/// Where an emptied scratch waits to be deleted, inside its room: moved
/// there at once, so a command the guest starts meanwhile gets a new one.
const EMPTIED: &str = "scratch.emptied";

fn empty_guest_scratch(node: &Arc<Node>, directory: &Path, now: jiff::Timestamp) -> io::Result<bool> {
    let emptied = directory.join(EMPTIED);
    if fs::exists(&emptied)? {
        // Left by a pass that stopped before deleting it.
        delete_owned(&emptied)?;
    }
    let scratch = directory.join("scratch");
    if !fs::is_dir(&scratch) || fastctx_platform::owned_directory::is_link(&fs::symlink_metadata(&scratch)?) {
        return Ok(false);
    }
    {
        // Claimed whole, no command of the room starts until it is moved
        // (`crate::jobs` holds its jobs and output directories to start
        // one), and none may be running or still be held.
        let Ok(_claimed) = node.job_files.collect(vec![directory.to_path_buf()]) else { return Ok(false) };
        if crate::job_index::running(&node.world)?.iter().any(|(_, path)| path.starts_with(directory))
            || now.duration_since(last_guest_activity(directory, now)?) < GUEST_SCRATCH_KEPT
        {
            return Ok(false);
        }
        fs::rename(&scratch, &emptied)?;
    }
    delete_owned(&emptied)?;
    Ok(true)
}

fn delete_owned(path: &Path) -> io::Result<()> {
    if fastctx_platform::owned_directory::is_link(&fs::symlink_metadata(path)?) {
        return fs::remove_file(path);
    }
    fastctx_platform::owned_directory::delete(path)
}

/// The last thing a guest did in its room: the room changed, or a command
/// it ran started or ended. A record that cannot be read counts as now.
fn last_guest_activity(directory: &Path, now: jiff::Timestamp) -> io::Result<jiff::Timestamp> {
    let mut last = jiff::Timestamp::try_from(fs::metadata(directory)?.modified()?).map_err(io::Error::other)?;
    for path in entries(&directory.join("jobs"))? {
        if path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        match record::read(&path) {
            Ok(job) => last = last.max(job.end.as_ref().map_or(job.started_at, |end| end.at.max(job.started_at))),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(_) => return Ok(now),
        }
    }
    Ok(last)
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

/// Lets go of the files of each ended job whose answer reached the model at
/// least the set days ago (`days`) and whose output is kept for good where
/// its conversation lives: in that conversation's scope here, whole; or, for
/// a conversation elsewhere, taken whole by its machine (`.collected`), or
/// waited for long enough. A job still read, still owed word of its end or
/// still needed by the activity book stays for a later pass.
fn expire(node: &Arc<Node>) -> io::Result<()> {
    let kept_for = i64::from(days(&node.world)?) * DAY_MS;
    let now = Timestamp::now().as_millisecond();
    let mut gone = 0usize;
    'parents: for parent in node.world.job_parents()? {
        for directory in entries(&parent)? {
            if fastctx_platform::owned_directory::is_link(&fs::symlink_metadata(&directory)?) {
                continue;
            }
            for path in entries(&directory.join("jobs"))? {
                if gone >= EXPIRED_AT_ONCE {
                    break 'parents;
                }
                if path.extension().is_none_or(|extension| extension != "json") {
                    continue;
                }
                if expire_one(node, &path, now, kept_for)? {
                    gone += 1;
                }
            }
        }
    }
    if gone > 0 {
        node.log.line(format_args!(
            "let go of the files of {gone} job(s) whose output is kept for good or waited long enough"
        ));
    }
    Ok(())
}

/// `expire` for the job whose record is at `path`: whether its files went.
fn expire_one(node: &Arc<Node>, path: &Path, now: i64, kept_for: i64) -> io::Result<bool> {
    let Some(held) = node.job_files.hold(path) else { return Ok(false) };
    let gate = node.job_files.gate(path);
    let _job = gate.lock();
    let job = match record::read(path) {
        Ok(job) => job,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    let Some(delivery) = crate::job_metadata::get::<JobDelivery>(&path.with_extension("delivered"))? else {
        return Ok(false);
    };
    let since = now.saturating_sub(delivery.at.as_millisecond());
    if since < kept_for
        || job.end.is_none()
        || program::in_use(&job)?
        || !crate::on::unfollowed(node, path)
        || crate::job_metadata::exists(&path.with_extension("owed"))?
    {
        return Ok(false);
    }
    let kept = if crate::job_assets::lives_here(node, &job) {
        crate::job_assets::kept_for_good(node, &job)
    } else {
        crate::job_metadata::exists(&path.with_extension("collected"))? || since >= UNCOLLECTED_DAYS * DAY_MS
    };
    if !kept {
        return Ok(false);
    }
    let Ok(_marked) = held.collect() else { return Ok(false) };
    match crate::job_watch::expire(node, path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(false),
        Err(error) => Err(error),
    }
}

/// How long a guest room stays after its guest last ran anything in it: by
/// then its jobs went (`expire`) and its scratch was emptied, and a deletion
/// its conversation's machine could not tell this one about owes it nothing.
const GUEST_ROOM_KEPT: jiff::SignedDuration = jiff::SignedDuration::from_hours(30 * 24);
/// The most guest rooms deleted in one pass.
const ROOMS_AT_ONCE: usize = 32;

/// Deletes the guest rooms whose jobs all went and whose guest ran nothing in
/// them for `GUEST_ROOM_KEPT`, as their deletion from every machine would;
/// what they hold of attachments goes by the attachments' own rule.
async fn expire_guest_rooms(node: &Arc<Node>) {
    let listing = node.clone();
    let rooms = match fastctx_platform::task::spawn_blocking(move || idle_guest_rooms(&listing)).await {
        Ok(Ok(rooms)) => rooms,
        Ok(Err(error)) => {
            node.log.line(format_args!("finding guest rooms nobody used for a month: {error}"));
            return;
        }
        Err(error) => {
            node.log.line(format_args!("finding guest rooms nobody used for a month: {error}"));
            return;
        }
    };
    for name in rooms.into_iter().take(ROOMS_AT_ONCE) {
        match crate::data::delete::delete_one(node, &name).await {
            Ok(_) => node.log.line(format_args!(
                "deleted guest room {name}: its guest ran nothing here for {} days",
                GUEST_ROOM_KEPT.as_hours() / 24
            )),
            Err(crate::data::delete::Stop::Nothing(_)) => {}
            Err(stop) => node.log.line(format_args!("deleting guest room {name}: {}", stop.why())),
        }
    }
}

fn idle_guest_rooms(node: &Arc<Node>) -> io::Result<Vec<String>> {
    let now = jiff::Timestamp::now();
    let running = crate::job_index::running(&node.world)?;
    let mut idle = Vec::new();
    for directory in entries(&node.world.conversations())? {
        let Some(name) = directory.file_name().and_then(|name| name.to_str()) else { continue };
        if !matches!(
            fastctx_types::layout::Kind::of(name),
            Some(fastctx_types::layout::Kind::Guest(_) | fastctx_types::layout::Kind::Visiting { .. })
        ) {
            continue;
        }
        if entries(&directory.join("jobs"))?
            .iter()
            .any(|path| path.extension().is_some_and(|extension| extension == "json"))
            || running.iter().any(|(_, path)| path.starts_with(&directory))
            || now.duration_since(last_guest_activity(&directory, now)?) < GUEST_ROOM_KEPT
        {
            continue;
        }
        idle.push(name.to_string());
    }
    Ok(idle)
}
