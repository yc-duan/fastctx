//! Old control claims are independent of the node's job records. Scan them
//! incrementally outside its disk lock, including on the first upgraded start.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use crate::Node;
use fastctx_platform::FileLock;

const BATCH: usize = 64;
const PAUSE: Duration = Duration::from_millis(10);
const EVERY: Duration = Duration::from_secs(60 * 60);
const SETTLED: Duration = Duration::from_secs(60 * 60);

pub(crate) fn start(node: &Arc<Node>) {
    let weak = Arc::downgrade(node);
    tokio::spawn(async move {
        let mut previous = String::new();
        loop {
            let Some(node) = weak.upgrade() else { return };
            let started = Instant::now();
            let collecting = node.clone();
            let mut pause = Duration::from_secs(60);
            match tokio::task::spawn_blocking(move || collect(&collecting.world)).await {
                Ok(Ok((count, busy))) => {
                    previous.clear();
                    pause = if count > 0 {
                        PAUSE
                    } else if busy > 0 {
                        Duration::from_secs(60)
                    } else {
                        EVERY
                    };
                    if count > 0 {
                        node.log.line(format_args!(
                            "collected {count} settled Claude control claims in {:.1} s; deferred {busy} locked claims",
                            started.elapsed().as_secs_f64()
                        ));
                    }
                }
                result => {
                    let reason = match result {
                        Ok(Err(error)) => error.to_string(),
                        Err(error) => error.to_string(),
                        _ => unreachable!(),
                    };
                    if reason != previous {
                        node.log.line(format_args!("collecting Claude control claims: {reason}; will retry"));
                        previous = reason;
                    }
                }
            }
            drop(node);
            // A nonblocking lock can defer a record, and a directory being
            // changed need not enumerate every entry in one pass (2026-10-29).
            // Revisit after progress instead of postponing upgrade leftovers.
            tokio::time::sleep(pause).await;
        }
    });
}

fn collect(world: &fastctx_types::layout::WorldDir) -> io::Result<(usize, usize)> {
    let mut roots = vec![fastctx_hosts::claude::exchange::sessions(world.path())];
    roots.extend(world.kept()?.into_iter().map(|kept| kept.directory.path().join("claude")));
    let mut count = 0;
    let mut busy = 0;
    for root in roots {
        let Some(sessions) = entries(&root)? else { continue };
        for session in sessions {
            let session = session?;
            // Aliases point at sessions visited through their conversation.
            if !session.file_type()?.is_dir() {
                continue;
            }
            count += claims(&session.path().join("control"), &mut busy)?;
        }
    }
    Ok((count, busy))
}

fn entries(path: &Path) -> io::Result<Option<std::fs::ReadDir>> {
    match std::fs::read_dir(path) {
        Ok(entries) => Ok(Some(entries)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn claims(control: &Path, busy: &mut usize) -> io::Result<usize> {
    let root = control.join("claims");
    let Some(entries) = entries(&root)? else { return Ok(0) };
    let mut batch = Vec::with_capacity(BATCH);
    let mut count = 0;
    for entry in entries {
        let entry = entry?;
        if entry.path().extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        batch.push(entry.path());
        if batch.len() == BATCH {
            count += retire(control, &batch, busy)?;
            batch.clear();
            std::thread::sleep(PAUSE);
        }
    }
    count += retire(control, &batch, busy)?;
    Ok(count)
}

fn retire(control: &Path, batch: &[PathBuf], busy: &mut usize) -> io::Result<usize> {
    let mut count = 0;
    for path in batch {
        // A conversation or exchange can disappear during this background scan.
        // Never create directories here or hold the job collector's disk lock.
        let lock = match FileLock::try_acquire(&control.join("claims/.lock")) {
            Ok(Some(lock)) => lock,
            Ok(None) => {
                *busy += 1;
                continue;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        let metadata = match std::fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        if !metadata.is_file() || SystemTime::now().duration_since(metadata.modified()?).unwrap_or_default() < SETTLED {
            continue;
        }
        let key = path.file_stem().expect("claim file");
        let mut asked = false;
        for side in ["inbox", "replies", "retained", "started"] {
            for extension in ["json", "done"] {
                asked |= control.join(side).join(key).with_extension(extension).try_exists()?;
            }
        }
        if !asked {
            match std::fs::remove_file(path) {
                Ok(()) => count += 1,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        drop(lock);
    }
    Ok(count)
}
