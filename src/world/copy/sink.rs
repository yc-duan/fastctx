//! The receiving end of a `copy`: accepts a `copy_receive` call by naming a stream, then, on
//! its own task, attaches to that stream, answers which files it already has, writes the rest
//! beside their final names, and renames each into place once its hash checks out.

use super::{ObjectKind, ReceiveArgs, safe_relative};
use crate::model::ToolResponse;
use crate::paths::{display_path, parse_input_path};
use crate::world::client::WorldClient;
use crate::world::messages::Call;
use crate::world::node::executor::VerbOutcome;
use crate::world::stream::{self, Control, Failure, Opener, Record, Sealer};
use crate::world::wire::StreamRole;
use futures_util::{SinkExt, StreamExt};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use tokio_tungstenite::tungstenite::Message;

/// Copies this machine receives at once.
const RECEIVE_LIMIT: usize = 4;

static ACTIVE: AtomicUsize = AtomicUsize::new(0);

struct FileInfo {
    target: PathBuf,
    relative: String,
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

#[derive(Default)]
struct Tally {
    wanted: u64,
    finalized: u64,
    copied: u64,
    bytes: u64,
    skipped: u64,
    links_skipped: u64,
    failed: Vec<Failure>,
    ended: bool,
}

/// Answers `copy_receive`: checks the destination, names a stream, and starts receiving. The
/// call's own timeout does not bound the transfer; the source's declared deadline does.
pub(crate) fn accept(client: &Arc<WorldClient>, call: &Call, _timeout: Duration) -> VerbOutcome {
    let args: ReceiveArgs = match serde_json::from_value(call.args.clone()) {
        Ok(args) => args,
        Err(error) => {
            return VerbOutcome::error_text(format!("Invalid copy_receive arguments: {error}"));
        }
    };
    let to = parse_input_path(&args.to);
    let shown = display_path(&to);
    if !to.is_absolute() {
        return VerbOutcome::error_text(format!(
            "to must be an absolute path on the receiving machine: {shown}"
        ));
    }
    match (std::fs::symlink_metadata(&to), args.kind) {
        (Ok(metadata), ObjectKind::File) if metadata.is_dir() => {
            return VerbOutcome::error_text(format!(
                "{shown} is a directory; to must name the file."
            ));
        }
        (Ok(metadata), ObjectKind::Dir) if !metadata.is_dir() => {
            return VerbOutcome::error_text(format!(
                "{shown} is a file; to must name the directory that receives the tree."
            ));
        }
        (Err(error), _) if error.kind() != std::io::ErrorKind::NotFound => {
            return VerbOutcome::error_text(format!("Cannot use {shown}: {error}"));
        }
        _ => {}
    }
    let created = match args.kind {
        ObjectKind::Dir => std::fs::create_dir_all(&to),
        ObjectKind::File => match to.parent() {
            Some(parent) => std::fs::create_dir_all(parent),
            None => Ok(()),
        },
    };
    if let Err(error) = created {
        return VerbOutcome::error_text(format!("Cannot create {shown}: {error}"));
    }
    if ACTIVE.load(Ordering::Relaxed) >= RECEIVE_LIMIT {
        return VerbOutcome::error_text(format!(
            "busy: this machine is receiving {RECEIVE_LIMIT} copies already; retry shortly."
        ));
    }
    let stream_id = match crate::world::crypto::random_bytes::<8>() {
        Ok(bytes) => u64::from_be_bytes(bytes).max(1),
        Err(error) => return VerbOutcome::error_text(error),
    };
    let epoch = client.keys.read().current().epoch();
    let key = match stream::blob_key(client, epoch) {
        Ok(key) => key,
        Err(error) => return VerbOutcome::error_text(error),
    };
    let deadline = Instant::now()
        + Duration::from_millis(args.deadline_ms).clamp(
            Duration::from_secs(5),
            Duration::from_millis(crate::shell::MAX_SCRIPT_CALL_MS),
        );
    let client = Arc::clone(client);
    tokio::spawn(async move {
        ACTIVE.fetch_add(1, Ordering::Relaxed);
        receive(client, stream_id, key, to, args, deadline).await;
        ACTIVE.fetch_sub(1, Ordering::Relaxed);
    });
    VerbOutcome {
        response: ToolResponse::text("receiving"),
        facts: BTreeMap::from([
            ("stream".to_string(), serde_json::json!(stream_id)),
            ("epoch".to_string(), serde_json::json!(epoch)),
            ("path".to_string(), serde_json::json!(shown)),
        ]),
    }
}

async fn receive(
    client: Arc<WorldClient>,
    stream_id: u64,
    key: crate::world::crypto::Key32,
    to: PathBuf,
    args: ReceiveArgs,
    deadline: Instant,
) {
    let dialed = match stream::open(&client, stream_id, StreamRole::Sink).await {
        Ok(dialed) => dialed,
        Err(error) => {
            crate::world::node::log(format!(
                "copy from \"{}\": cannot open the data connection: {error}",
                args.from_node
            ));
            return;
        }
    };
    let (mut writer, mut reader) = dialed.socket.split();
    let mut sealer = Sealer::new(key, stream_id, StreamRole::Sink);
    let mut opener = Opener::new(key, stream_id, StreamRole::Source);
    let mut pending: HashMap<u64, FileInfo> = HashMap::new();
    let mut current: Option<Current> = None;
    let mut tally = Tally::default();
    let deadline = tokio::time::Instant::from_std(deadline);
    let outcome: Result<(), String> = loop {
        let bytes = tokio::select! {
            () = tokio::time::sleep_until(deadline) => break Err("the copy ran past its deadline".to_string()),
            next = reader.next() => match next {
                None | Some(Ok(Message::Close(_))) => break Err("the source closed the data connection".to_string()),
                Some(Err(error)) => break Err(format!("the data connection failed: {error}")),
                Some(Ok(Message::Binary(bytes))) => bytes,
                Some(Ok(_)) => continue,
            },
        };
        let record = match opener.open(&bytes) {
            Ok(record) => record,
            Err(error) => break Err(error),
        };
        let reply = match record {
            Record::Control(Control::File {
                index,
                path,
                size,
                mode,
                mtime,
                sha256,
            }) => {
                let target = match args.kind {
                    ObjectKind::File => Ok(to.clone()),
                    ObjectKind::Dir => safe_relative(&path).map(|components| {
                        let mut target = to.clone();
                        for component in components {
                            target.push(component);
                        }
                        target
                    }),
                };
                match target {
                    Err(reason) => {
                        tally.failed.push(Failure { path, reason });
                        Some(Control::Skip { index })
                    }
                    Ok(target) => {
                        let identical = same_content(&target, size, &sha256).await;
                        if identical {
                            tally.skipped += 1;
                            Some(Control::Skip { index })
                        } else {
                            tally.wanted += 1;
                            pending.insert(
                                index,
                                FileInfo {
                                    target,
                                    relative: path,
                                    size,
                                    mode,
                                    mtime,
                                    sha256,
                                },
                            );
                            Some(Control::Want { index })
                        }
                    }
                }
            }
            Record::Control(Control::Dir { path }) => {
                if args.kind == ObjectKind::Dir {
                    match safe_relative(&path) {
                        Ok(components) => {
                            let mut target = to.clone();
                            for component in components {
                                target.push(component);
                            }
                            if let Err(error) = std::fs::create_dir_all(&target) {
                                tally.failed.push(Failure {
                                    path,
                                    reason: format!("cannot create the directory: {error}"),
                                });
                            }
                        }
                        Err(reason) => tally.failed.push(Failure { path, reason }),
                    }
                }
                None
            }
            Record::Control(Control::LinkSkipped { .. }) => {
                tally.links_skipped += 1;
                None
            }
            Record::Control(Control::FileBegin { index }) => {
                if let Some(previous) = current.take() {
                    discard(previous);
                }
                if let Some(info) = pending.get(&index) {
                    match open_temporary(&info.target) {
                        Ok((temporary, file)) => {
                            current = Some(Current {
                                index,
                                temporary,
                                file,
                                hasher: Sha256::new(),
                                written: 0,
                            });
                        }
                        Err(reason) => {
                            tally.failed.push(Failure {
                                path: info.relative.clone(),
                                reason,
                            });
                        }
                    }
                }
                None
            }
            Record::Data(data) => {
                if let Some(state) = current.as_mut() {
                    if let Err(error) = state.file.write_all(&data) {
                        let info = pending.get(&state.index);
                        tally.failed.push(Failure {
                            path: info.map(|info| info.relative.clone()).unwrap_or_default(),
                            reason: format!("cannot write: {error}"),
                        });
                        if let Some(state) = current.take() {
                            discard(state);
                        }
                    } else {
                        state.hasher.update(&data);
                        state.written += data.len() as u64;
                    }
                }
                None
            }
            Record::Control(Control::FileEnd { index }) => {
                tally.finalized += 1;
                match (current.take(), pending.remove(&index)) {
                    (Some(state), Some(info)) if state.index == index => {
                        match finalize(state, &info).await {
                            Ok(written) => {
                                tally.copied += 1;
                                tally.bytes += written;
                                Some(Control::Received {
                                    index,
                                    bytes: written,
                                })
                            }
                            Err(reason) => {
                                tally.failed.push(Failure {
                                    path: info.relative,
                                    reason,
                                });
                                None
                            }
                        }
                    }
                    (state, info) => {
                        if let Some(state) = state {
                            discard(state);
                        }
                        if let Some(info) = info
                            && !tally
                                .failed
                                .iter()
                                .any(|failure| failure.path == info.relative)
                        {
                            tally.failed.push(Failure {
                                path: info.relative,
                                reason: "the file did not arrive whole".to_string(),
                            });
                        }
                        None
                    }
                }
            }
            Record::Control(Control::End { .. }) => {
                tally.ended = true;
                None
            }
            Record::Control(_) => None,
        };
        if let Some(reply) = reply
            && let Err(error) = send(&mut writer, &mut sealer, reply).await
        {
            break Err(error);
        }
        if tally.ended && tally.finalized >= tally.wanted {
            let done = Control::Done {
                copied: tally.copied,
                bytes: tally.bytes,
                skipped: tally.skipped,
                links_skipped: tally.links_skipped,
                failed: std::mem::take(&mut tally.failed),
            };
            break send(&mut writer, &mut sealer, done).await;
        }
    };
    if let Some(state) = current.take() {
        discard(state);
    }
    let _ = writer.close().await;
    match outcome {
        Ok(()) => crate::world::node::log(format!(
            "copy from \"{}\" into {}: {} files, {} bytes",
            args.from_node,
            display_path(&to),
            tally.copied,
            tally.bytes
        )),
        Err(error) => crate::world::node::log(format!(
            "copy from \"{}\" into {} stopped after {} files: {error}",
            args.from_node,
            display_path(&to),
            tally.copied
        )),
    }
}

async fn send(
    writer: &mut futures_util::stream::SplitSink<crate::world::link::ws::Socket, Message>,
    sealer: &mut Sealer,
    control: Control,
) -> Result<(), String> {
    let bytes = sealer.seal(&Record::Control(control))?;
    writer
        .send(Message::Binary(bytes.into()))
        .await
        .map_err(|error| format!("the data connection closed while replying: {error}"))
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
        crate::world::remote_edit::sha256_of(&target).is_ok_and(|actual| actual == expected)
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
    if let Ok(entries) = std::fs::read_dir(parent) {
        for entry in entries.flatten() {
            if entry
                .file_name()
                .to_string_lossy()
                .starts_with(&stale_prefix)
            {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
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
