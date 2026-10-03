//! A copy receiver publishes files only after the final cumulative stream signature verifies.
use super::{ObjectKind, ReceiveArgs, safe_relative};
use crate::model::ToolResponse;
use crate::paths::{display_path, parse_input_path};
use crate::world::{
    client::WorldClient,
    messages::Call,
    node::executor::VerbOutcome,
    stream::{self, Control, Record},
    wire::StreamRole,
};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
struct FileInfo {
    target: PathBuf,
    size: u64,
    mode: Option<u32>,
    mtime: Option<i64>,
    sha256: String,
}

struct Current {
    index: u64,
    temporary: PathBuf,
    file: std::fs::File,
    hasher: Sha256,
    written: u64,
}

pub(crate) fn accept(client: &Arc<WorldClient>, call: &Call, _timeout: Duration) -> VerbOutcome {
    match prepare(client, call) {
        Ok(outcome) => outcome,
        Err(error) => VerbOutcome::error_text(error),
    }
}
fn prepare(client: &Arc<WorldClient>, call: &Call) -> Result<VerbOutcome, String> {
    let args: ReceiveArgs = serde_json::from_value(call.args.clone()).map_err(|e| e.to_string())?;
    let to = parse_input_path(&args.to);
    if !to.is_absolute() {
        return Err("copy destination must be absolute".into());
    }
    if args.ticket.source != args.from_node
        || args.ticket.sink != client.name()
        || args.ticket.verb != "copy"
        || args.ticket.principal != args.from_node
        || args.ticket.target != client.name()
    {
        return Err("copy ticket does not authorize these endpoints".into());
    }
    let key = client
        .members
        .read()
        .trusted_key(&args.from_node)
        .ok_or("Untrusted copy source")?;
    args.ticket.verify(&key)?;
    let client = Arc::clone(client);
    let shown = display_path(&to);
    tokio::spawn(async move {
        let result = tokio::time::timeout(
            Duration::from_millis(args.deadline_ms),
            receive(&client, &to, &args),
        )
        .await;
        match result {
            Ok(Ok(())) => {}
            Ok(Err(error)) => crate::world::node::log(format!("copy rejected: {error}")),
            Err(_) => crate::world::node::log("copy exceeded caller deadline"),
        }
    });
    Ok(VerbOutcome {
        response: ToolResponse::text("receiving"),
        facts: BTreeMap::from([("path".into(), serde_json::json!(shown))]),
    })
}
async fn receive(client: &WorldClient, to: &Path, args: &ReceiveArgs) -> Result<(), String> {
    let dialed = stream::open(client, &args.ticket, StreamRole::Sink).await?;
    let mut channel = stream::Channel::new(client, dialed, args.ticket.clone(), StreamRole::Sink)?;
    let mut staged: Vec<(Current, FileInfo)> = Vec::new();
    let mut pending: Option<FileInfo> = None;
    let mut current: Option<Current> = None;
    let mut directories = Vec::new();
    let (mut skipped, mut links, mut files, mut total_bytes) = (0, 0, 0, 0);
    let outcome = async {
        loop {
            match channel.recv().await? {
                Record::Control(Control::File {
                    index,
                    path,
                    size,
                    mode,
                    mtime,
                    sha256,
                }) => {
                    if pending.is_some() || current.is_some() {
                        return Err("overlapping stream files".into());
                    }
                    let target = if args.kind == ObjectKind::File {
                        to.to_path_buf()
                    } else {
                        safe_relative(&path)?
                            .iter()
                            .fold(to.to_path_buf(), |p, c| p.join(c))
                    };
                    files += 1;
                    total_bytes += size;
                    if same_content(&target, size, &sha256).await {
                        skipped += 1;
                        channel
                            .send(Record::Control(Control::Skip { index }))
                            .await?;
                    } else {
                        pending = Some(FileInfo {
                            target,
                            size,
                            mode,
                            mtime,
                            sha256,
                        });
                        channel
                            .send(Record::Control(Control::Want { index }))
                            .await?;
                    }
                }
                Record::Control(Control::FileBegin { index }) => {
                    let info = pending.as_ref().ok_or("file began without a manifest")?;
                    let (temporary, file) = open_temporary(&info.target)?;
                    current = Some(Current {
                        index,
                        temporary,
                        file,
                        hasher: Sha256::new(),
                        written: 0,
                    });
                }
                Record::Data(data) => {
                    let state = current.as_mut().ok_or("bytes outside a file")?;
                    state.file.write_all(&data).map_err(|e| e.to_string())?;
                    state.hasher.update(&data);
                    state.written += data.len() as u64;
                }
                Record::Control(Control::FileEnd { index }) => {
                    let state = current.take().ok_or("file end without file")?;
                    if state.index != index {
                        return Err("file index mismatch".into());
                    }
                    staged.push((state, pending.take().ok_or("file has no manifest")?));
                }
                Record::Control(Control::Dir { path }) => directories.push(
                    safe_relative(&path)?
                        .iter()
                        .fold(to.to_path_buf(), |p, c| p.join(c)),
                ),
                Record::Control(Control::LinkSkipped { .. }) => links += 1,
                Record::Control(Control::End {
                    files: expected_files,
                    bytes,
                }) => {
                    if expected_files != files
                        || bytes != total_bytes
                        || current.is_some()
                        || pending.is_some()
                    {
                        return Err("incomplete stream".into());
                    }
                    break;
                }
                _ => return Err("unexpected copy stream record".into()),
            }
        }
        // The final record authenticates the complete transcript, including every byte and
        // both roles via the ticket. Nothing becomes a destination file before this point.
        for directory in directories {
            std::fs::create_dir_all(directory).map_err(|e| e.to_string())?;
        }
        let (mut copied, mut bytes) = (0, 0);
        for (state, info) in staged.drain(..) {
            bytes += finalize(state, &info).await?;
            copied += 1;
        }
        channel
            .send(Record::Control(Control::Done {
                copied,
                bytes,
                skipped,
                links_skipped: links,
                failed: Vec::new(),
            }))
            .await
    }
    .await;
    for (state, _) in staged {
        discard(state);
    }
    if let Some(state) = current {
        discard(state);
    }
    outcome
}
/// Whether the file already at `target` has the manifest's size and hash.
async fn same_content(target: &Path, size: u64, sha256: &str) -> bool {
    let Ok(metadata) = std::fs::symlink_metadata(target) else {
        return false;
    };
    if !metadata.is_file() || metadata.len() != size {
        return false;
    }
    let target = target.to_path_buf();
    let expected = sha256.to_string();
    tokio::task::spawn_blocking(move || {
        crate::world::file_hash::sha256_of(&target).is_ok_and(|actual| actual == expected)
    })
    .await
    .unwrap_or(false)
}

/// Opens the temporary file a copy is written to, beside its final name so the rename that
/// completes it never crosses a mount point. A part file left there by a copy that died
/// with its node (a killed process never reaches `discard`) is removed first, so running the
/// copy again leaves nothing behind.
fn open_temporary(target: &Path) -> Result<(PathBuf, std::fs::File), String> {
    let parent = target
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", display_path(target)))?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("cannot create {}: {error}", display_path(parent)))?;
    let name = target
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".to_string());
    let stale_prefix = format!(".{name}.fastctx-part-");
    let suffix = crate::world::crypto::random_bytes::<3>()
        .map(hex::encode)
        .unwrap_or_else(|_| std::process::id().to_string());
    let temporary = parent.join(format!("{stale_prefix}{suffix}"));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options
        .open(&temporary)
        .map_err(|error| format!("cannot create {}: {error}", display_path(&temporary)))?;
    Ok((temporary, file))
}

/// Verifies, flushes, stamps, and renames one received file into place.
async fn finalize(state: Current, info: &FileInfo) -> Result<u64, String> {
    let Current {
        temporary,
        file,
        hasher,
        written,
        ..
    } = state;
    let actual = hex::encode(hasher.finalize());
    if written != info.size || actual != info.sha256 {
        let _ = std::fs::remove_file(&temporary);
        return Err(if written != info.size {
            format!("received {written} of {} bytes", info.size)
        } else {
            "the content changed while it was being copied".to_string()
        });
    }
    let target = info.target.clone();
    let mode = info.mode;
    let mtime = info.mtime;
    let temporary_for_task = temporary.clone();
    let result = tokio::task::spawn_blocking(move || -> Result<(), String> {
        file.sync_all().map_err(|error| {
            format!(
                "cannot flush {}: {error}",
                display_path(&temporary_for_task)
            )
        })?;
        // The temporary file was created private; the finished file gets the source's mode, or
        // an ordinary 0644 when the source (a Windows machine) had none to report.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = file.set_permissions(std::fs::Permissions::from_mode(mode.unwrap_or(0o644)));
        }
        #[cfg(not(unix))]
        let _ = mode;
        if let Some(mtime) = mtime
            && let Some(time) =
                std::time::UNIX_EPOCH.checked_add(Duration::from_secs(mtime.max(0) as u64))
        {
            let _ = file.set_modified(time);
        }
        drop(file);
        std::fs::rename(&temporary_for_task, &target).map_err(|error| {
            format!(
                "cannot move into place at {}: {error}",
                display_path(&target)
            )
        })
    })
    .await
    .map_err(|error| format!("Internal copy failure: {error}"))?;
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result.map(|()| written)
}

fn discard(state: Current) {
    drop(state.file);
    let _ = std::fs::remove_file(&state.temporary);
}
