//! Live picture locators wait for the host to flush their own record.

use super::image_records::paths;
use fastctx_hosts::claude::exchange;
use fastctx_types::ResidentId;
use std::io::{Read as _, Seek as _};

/// The mod observes append before Claude flushes its JSONL row. Publish a
/// picture locator only once that row is readable, or report the host's exit.
pub(crate) async fn wait_for_image(node: &crate::Node, resident: &ResidentId, message: &str) -> std::io::Result<()> {
    let paths = paths(node, resident).map_err(|error| std::io::Error::other(error.message))?;
    let (changed, mut changes) = tokio::sync::mpsc::unbounded_channel();
    let mut watcher = fastctx_watch::Watcher::new(move |event: notify::Result<notify::Event>| {
        if crate::file_events::may_change(&event) {
            let _ = changed.send(event.map(|_| ()));
        }
    });
    for parent in paths.iter().filter_map(|path| path.parent()) {
        watcher.watch(parent, notify::RecursiveMode::NonRecursive).map_err(std::io::Error::other)?;
    }
    let process = node
        .residents
        .about(resident)
        .and_then(|(about, _)| about.session)
        .map(|session| exchange::registration(&exchange::sessions(node.world.path()).join(session)))
        .transpose()?
        .flatten()
        .and_then(|registration| registration.process);
    let mut ended: std::pin::Pin<Box<dyn std::future::Future<Output = std::io::Result<()>> + Send>> = match process {
        Some(process) => Box::pin(async move { fastctx_platform::host_process::exited(&process).await }),
        None => Box::pin(async {
            Err(std::io::Error::other(
                "Claude has not retained the user picture's message and did not identify its process",
            ))
        }),
    };
    let temporary = node.world.image_reading();
    fastctx_platform::ensure_private_dir(&temporary)?;
    let scope = tempfile::tempdir_in(temporary)?;
    let mut host_ended = false;
    loop {
        let paths = paths.clone();
        let message = message.to_owned();
        let temporary = scope.path().to_owned();
        if tokio::task::spawn_blocking(move || {
            for path in paths {
                if contains(&path, &message, &temporary)? {
                    return Ok(true);
                }
            }
            Ok::<_, std::io::Error>(false)
        })
        .await
        .map_err(std::io::Error::other)??
        {
            return Ok(());
        }
        if host_ended {
            return Err(std::io::Error::other("Claude ended before retaining the user picture's message"));
        }
        tokio::select! {
            event = changes.recv() => event.ok_or_else(|| std::io::Error::other("the Claude image-record watcher ended"))?.map_err(std::io::Error::other)?,
            result = &mut ended => {
                result?;
                host_ended = true;
            }
        }
    }
}

fn contains(path: &std::path::Path, message: &str, temporary: &std::path::Path) -> std::io::Result<bool> {
    let mut file = match fastctx_platform::file_read::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    let mut end = file.metadata()?.len();
    let mut tail = [0; 8192];
    // A partial final JSONL row belongs to the host's next flush. Earlier
    // complete rows remain readable without waiting on a moving EOF.
    while end > 0 {
        let start = end.saturating_sub(tail.len() as u64);
        file.seek(std::io::SeekFrom::Start(start))?;
        let count = (end - start) as usize;
        file.read_exact(&mut tail[..count])?;
        if let Some(newline) = tail[..count].iter().rposition(|byte| *byte == b'\n') {
            end = start + newline as u64 + 1;
            break;
        }
        end = start;
    }
    file.rewind()?;
    let cancelled = std::sync::atomic::AtomicBool::new(false);
    let mut reader = std::io::BufReader::new(file.take(end));
    while let Some(record) = fastctx_hosts::image_record::next(&mut reader, temporary, &cancelled)? {
        if record.value["type"] == "user" && record.value["uuid"] == message {
            return Ok(true);
        }
    }
    Ok(false)
}
