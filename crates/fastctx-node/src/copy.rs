//! `fastctx copy`: files from one machine to another (`fastctx_types::copy`).
//!
//! Whoever is asked to put files somewhere answers for it as itself, under
//! its own scope, since that is all the machine written to can check. So
//! the machine asked, when it holds the source, sends: it walks its path and
//! calls the destination with a data connection, and the destination admits
//! it by its scope. When it is the destination, it fetches: it calls the
//! source, which admits it by its scope and sends back over the call's data
//! connection. Asked to copy between two other machines, it has the source
//! send, so the source's scope must reach the destination.
//!
//! Over the connection go the manifest, then the destination's answer of
//! which files it needs, then those files' bytes, each checked against the
//! manifest's hash before it is put in place. The connection is sealed with
//! a key only the two ends hold; a sent manifest is pinned besides by the
//! digest the signed call carries.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use fastctx_hands::copy::{Receiver, walk};
use fastctx_net::data::Stream;
use fastctx_net::link::{CallFailure, Feed, Link};
use fastctx_types::MachineName;
use fastctx_types::copy::{CopyReport, EntryKind, Manifest, Place};
use fastctx_types::ipc::Failure;
use fastctx_types::remote::{RemoteCall, RemoteReply};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use tokio::io::AsyncReadExt as _;

use crate::Node;

/// What the destination says once it has the manifest.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
enum Answer {
    /// Send these entries' files, in this order.
    Want {
        files: Vec<usize>,
    },
    Refused {
        reason: String,
    },
}

/// Copies `from` to `to`, asked of this machine.
pub async fn copy(node: &Arc<Node>, from: Place, to: Place) -> Result<CopyReport, Failure> {
    let here = node.machine();
    let from_machine = from.machine.unwrap_or_else(|| here.clone());
    let to_machine = to.machine.unwrap_or_else(|| here.clone());
    if from_machine == to_machine {
        return Err(Failure::new(format!(
            "both ends are on {from_machine}; copy within one machine with cp, in a run on it"
        )));
    }
    if from_machine == here {
        return send(node, &from.path, &to_machine, &to.path).await;
    }
    if to_machine == here {
        return fetch(node, &from_machine, &from.path, &to.path).await;
    }
    let link = link(node)?;
    let call = RemoteCall::CopySend { from: from.path, to: Place { machine: Some(to_machine), path: to.path } };
    match link.call(&from_machine, call).await.map_err(|failure| Failure::new(failure.to_string()))? {
        Ok(RemoteReply::Copied(report)) => Ok(report),
        Ok(_) => Err(Failure::new(format!("{from_machine} answered something other than a copy"))),
        Err(failure) => Err(failure),
    }
}

fn link(node: &Node) -> Result<Arc<Link>, Failure> {
    node.link().ok_or_else(|| Failure::new(format!("{} is in no World with other machines", node.machine())))
}

/// What a source has to send: its manifest, the files behind it, and what
/// it could not read.
struct Walked {
    manifest: Vec<u8>,
    files: Vec<(PathBuf, Option<u64>)>,
    problems: Vec<String>,
}

/// Walks `from`, on this machine, for sending.
async fn walked(node: &Node, from: &str) -> Result<Walked, Failure> {
    let root = fastctx_platform::local_path(from).map_err(|error| Failure::new(error.to_string()))?;
    if !root.is_absolute() {
        return Err(Failure::new(format!("{from} is not an absolute path on {}", node.machine())));
    }
    let walked = tokio::task::spawn_blocking(move || walk(&root))
        .await
        .map_err(|error| Failure::new(format!("reading {from}: {error}")))?
        .map_err(|error| Failure::new(format!("{from} on {}: {error}", node.machine())))?;
    let manifest = serde_json::to_vec(&walked.manifest).expect("manifests serialize");
    let files = walked
        .sources
        .iter()
        .zip(&walked.manifest.entries)
        .map(|(path, entry)| match entry.kind {
            EntryKind::File { size, .. } => (path.clone(), Some(size)),
            _ => (path.clone(), None),
        })
        .collect();
    Ok(Walked { manifest, files, problems: walked.problems })
}

/// What a source that is fetched from says first: how long its manifest is.
#[derive(Serialize, Deserialize)]
struct Offer {
    manifest_bytes: u64,
}

/// The source's side when it is fetched from: walks `from`, here, and sends
/// it back over the caller's data connection.
pub async fn give(node: &Arc<Node>, from: &str, mut stream: Stream) -> Result<CopyReport, Failure> {
    let Walked { manifest, files, problems } = walked(node, from).await?;
    stream.say(&Offer { manifest_bytes: manifest.len() as u64 }).await.map_err(Failure::new)?;
    feed(stream, manifest, files).await.map_err(Failure::new)?;
    Ok(CopyReport {
        from: slashed(Path::new(from)),
        from_machine: node.machine(),
        to: String::new(),
        to_machine: node.machine(),
        files: 0,
        bytes: 0,
        directories: 0,
        links: 0,
        unchanged: 0,
        problems,
    })
}

/// The destination's side when it is the machine asked: calls `from_machine`
/// for `from` and puts it at `to`, here.
async fn fetch(node: &Arc<Node>, from_machine: &MachineName, from: &str, to: &str) -> Result<CopyReport, Failure> {
    let link = link(node)?;
    // The feed keeps what it took before it sends its receipt; the source
    // answers only after the receipt, so the answer never comes first.
    let taken: Arc<std::sync::Mutex<Option<CopyReport>>> = Arc::default();
    let (here, to, kept) = (node.clone(), to.to_string(), taken.clone());
    let feed: Feed = Box::new(move |mut stream| {
        Box::pin(async move {
            let offer: Offer = stream.hear().await?;
            let report = take_files(&here, to, offer.manifest_bytes, None, &mut stream)
                .await
                .map_err(|failure| failure.message)?;
            *kept.lock().expect("never poisoned") = Some(report);
            stream.done().await
        })
    });
    let call = RemoteCall::CopyFetch { from: from.to_string() };
    let answer = link.call_with_stream(from_machine, call, feed).await.map_err(|failure| match failure {
        CallFailure::Unknown { name, reason } if reason.starts_with("sending its data failed: ") => {
            Failure::new(format!("the copy from {name} stopped: {}", &reason["sending its data failed: ".len()..]))
        }
        other => Failure::new(other.to_string()),
    })?;
    let sent = match answer {
        Ok(RemoteReply::Copied(report)) => report,
        Ok(_) => return Err(Failure::new(format!("{from_machine} answered something other than a copy"))),
        Err(failure) => return Err(failure),
    };
    let mut report = taken
        .lock()
        .expect("never poisoned")
        .take()
        .ok_or_else(|| Failure::new(format!("{from_machine} answered before the copy had arrived")))?;
    report.from = sent.from;
    report.from_machine = sent.from_machine;
    let mut problems = sent.problems;
    problems.append(&mut report.problems);
    report.problems = problems;
    Ok(report)
}

/// The source's side: walks `from`, here, and sends it to `to` on `to_machine`.
pub async fn send(node: &Arc<Node>, from: &str, to_machine: &MachineName, to: &str) -> Result<CopyReport, Failure> {
    let link = link(node)?;
    let Walked { manifest, files, problems: walk_problems } = walked(node, from).await?;
    let call = RemoteCall::CopyReceive {
        to: to.to_string(),
        manifest_bytes: manifest.len() as u64,
        manifest_sha256: format!("{:x}", Sha256::digest(&manifest)),
    };
    let feed: Feed = Box::new(move |stream| Box::pin(feed(stream, manifest, files)));
    let answer = link.call_with_stream(to_machine, call, feed).await.map_err(|failure| match failure {
        CallFailure::Unknown { name, reason } if reason.starts_with("sending its data failed: ") => {
            Failure::new(format!("the copy to {name} stopped: {}", &reason["sending its data failed: ".len()..]))
        }
        other => Failure::new(other.to_string()),
    })?;
    let mut report = match answer {
        Ok(RemoteReply::Copied(report)) => report,
        Ok(_) => return Err(Failure::new(format!("{to_machine} answered something other than a copy"))),
        Err(failure) => return Err(failure),
    };
    report.from = slashed(Path::new(from));
    report.from_machine = node.machine();
    let mut problems = walk_problems;
    problems.append(&mut report.problems);
    report.problems = problems;
    Ok(report)
}

/// A path as the World writes it: on Windows with forward slashes.
fn slashed(path: &Path) -> String {
    let text = path.to_string_lossy();
    if cfg!(windows) { text.replace('\\', "/") } else { text.into_owned() }
}

/// The source's end of the data connection: the manifest, then the files
/// the destination wants.
async fn feed(mut stream: Stream, manifest: Vec<u8>, files: Vec<(PathBuf, Option<u64>)>) -> Result<(), String> {
    stream.send(&manifest).await?;
    let wanted = match stream.hear::<Answer>().await? {
        Answer::Want { files } => files,
        Answer::Refused { reason } => return Err(reason),
    };
    let mut buffer = vec![0u8; 1 << 20];
    for index in wanted {
        let Some((path, Some(size))) = files.get(index) else {
            return Err(format!("the destination asked for entry {index}, which is not a file of this copy"));
        };
        send_file(&mut stream, path, *size, &mut buffer).await?;
    }
    stream.finish().await
}

/// Sends exactly `size` bytes of a file. A file that has shrunk since it was
/// hashed, or cannot be read, is padded with zeros to its size, so the
/// transfer stays in step; its hash then shows the destination it is not
/// the file promised, and it is not kept.
async fn send_file(stream: &mut Stream, path: &Path, size: u64, buffer: &mut [u8]) -> Result<(), String> {
    let mut left = size;
    if let Ok(mut file) = tokio::fs::File::open(path).await {
        while left > 0 {
            let want = (buffer.len() as u64).min(left) as usize;
            match file.read(&mut buffer[..want]).await {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    stream.send(&buffer[..read]).await?;
                    left -= read as u64;
                }
            }
        }
    }
    buffer.fill(0);
    while left > 0 {
        let want = (buffer.len() as u64).min(left) as usize;
        stream.send(&buffer[..want]).await?;
        left -= want as u64;
    }
    Ok(())
}

/// The destination's side when the source calls: takes the copy, and says
/// it has everything.
pub async fn receive(
    node: &Node,
    to: String,
    manifest_bytes: u64,
    manifest_sha256: String,
    mut stream: Stream,
) -> Result<CopyReport, Failure> {
    let report = take_files(node, to, manifest_bytes, Some(&manifest_sha256), &mut stream).await?;
    stream.done().await.map_err(Failure::new)?;
    Ok(report)
}

/// Takes the manifest, checked against `manifest_sha256` when the source
/// signed for one, says which files it needs, and puts each in place as it
/// comes; the receipt is the caller's to send.
async fn take_files(
    node: &Node,
    to: String,
    manifest_bytes: u64,
    manifest_sha256: Option<&str>,
    stream: &mut Stream,
) -> Result<CopyReport, Failure> {
    let failure = |reason: String| Failure::new(reason);
    let mut json = Vec::new();
    stream
        .take(manifest_bytes, |piece| {
            json.extend_from_slice(piece);
            Ok(())
        })
        .await
        .map_err(failure)?;
    if manifest_sha256.is_some_and(|signed| format!("{:x}", Sha256::digest(&json)) != signed) {
        return Err(Failure::new("the manifest is not the one the source signed for"));
    }
    let manifest: Manifest =
        serde_json::from_slice(&json).map_err(|error| Failure::new(format!("the manifest cannot be read: {error}")))?;
    let receiver = match Receiver::new(&to, manifest) {
        Ok(receiver) => receiver,
        Err(reason) => {
            let _ = stream.say(&Answer::Refused { reason: reason.clone() }).await;
            return Err(Failure::new(reason));
        }
    };
    // Looking at what is there already reads every file it has.
    let (mut receiver, wanted) = tokio::task::spawn_blocking(move || {
        let mut receiver = receiver;
        let wanted = receiver.wanted();
        (receiver, wanted)
    })
    .await
    .map_err(|error| Failure::new(format!("looking at the destination: {error}")))?;
    stream.say(&Answer::Want { files: wanted.clone() }).await.map_err(failure)?;
    for index in wanted {
        let (size, mut arriving) = receiver.arrive(index);
        stream.take(size, |piece| arriving.write(piece)).await.map_err(failure)?;
        receiver.arrived(index, arriving);
    }
    receiver.link();
    Ok(CopyReport {
        from: String::new(),
        from_machine: node.machine(),
        to: slashed(receiver.destination()),
        to_machine: node.machine(),
        files: receiver.files,
        bytes: receiver.bytes,
        directories: receiver.directories,
        links: receiver.links,
        unchanged: receiver.unchanged,
        problems: receiver.problems,
    })
}
