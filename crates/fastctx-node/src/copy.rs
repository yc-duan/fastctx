//! `fastctx copy`: files from one machine to another (`fastctx_types::copy`).
//!
//! The machine asked finds which machine holds the source and has it send:
//! the source walks its path and calls the destination with a data
//! connection. Over it go the manifest, which the call's signed digest
//! pins, then the destination's answer of which files it needs, then those
//! files' bytes, each checked against the manifest's hash before it is put
//! in place.

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

/// The source's side: walks `from`, here, and sends it to `to` on `to_machine`.
pub async fn send(node: &Arc<Node>, from: &str, to_machine: &MachineName, to: &str) -> Result<CopyReport, Failure> {
    let link = link(node)?;
    let root = PathBuf::from(from);
    if !root.is_absolute() {
        return Err(Failure::new(format!("{from} is not an absolute path on {}", node.machine())));
    }
    let walked = tokio::task::spawn_blocking(move || walk(&root))
        .await
        .map_err(|error| Failure::new(format!("reading {from}: {error}")))?
        .map_err(|error| Failure::new(format!("{from} on {}: {error}", node.machine())))?;
    let manifest = serde_json::to_vec(&walked.manifest).expect("manifests serialize");
    let call = RemoteCall::CopyReceive {
        to: to.to_string(),
        manifest_bytes: manifest.len() as u64,
        manifest_sha256: format!("{:x}", Sha256::digest(&manifest)),
    };
    let files: Vec<(PathBuf, Option<u64>)> = walked
        .sources
        .iter()
        .zip(&walked.manifest.entries)
        .map(|(path, entry)| match entry.kind {
            EntryKind::File { size, .. } => (path.clone(), Some(size)),
            _ => (path.clone(), None),
        })
        .collect();
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
    let mut problems = walked.problems;
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

/// The destination's side: takes the manifest, says which files it needs,
/// and puts each in place as it comes.
pub async fn receive(
    node: &Node,
    to: String,
    manifest_bytes: u64,
    manifest_sha256: String,
    mut stream: Stream,
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
    if format!("{:x}", Sha256::digest(&json)) != manifest_sha256 {
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
    stream.done().await.map_err(failure)?;
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
