//! The signed original-byte read shared by machine and device clients.
//!
//! A read whose way broke, a call no answer came to or a data connection
//! that ended, fails as unanswered (`Failure::is_unanswered`): its caller
//! goes on from the bytes that came (`world-network.md`, 断开不是结束).
//! Bytes are passed on only once their signed digest is checked, so what
//! came before a break is always what the machine sent.

use crate::link::{CallFailure, Feed, Link};
use crate::sign::{Digest, Domain, Signed, sha256};
use fastctx_types::access::Purpose;
use fastctx_types::file_read::*;
use fastctx_types::ipc::Failure;
use fastctx_types::remote::{RemoteCall, RemoteReply};
use fastctx_types::{AccountName, MachineName};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, oneshot};

pub enum Part {
    Opened(FileReadInfo),
    Chunk { offset: u64, bytes: Vec<u8> },
    Finished(FileReadResult),
}

pub type Sender = mpsc::Sender<Result<Part, Failure>>;

#[derive(Serialize, Deserialize)]
pub struct Piece {
    pub read: u64,
    pub n: u64,
    pub part: Content,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Content {
    Opened { info: FileReadInfo },
    Chunk { offset: u64, bytes: u64, sha256: Digest },
    Finished { result: FileReadResult },
}

pub async fn read(
    link: &Arc<Link>,
    machine: MachineName,
    account: AccountName,
    request: ReadFileRequest,
    sender: &Sender,
) -> Result<FileReadResult, Failure> {
    let path = Some(request.path.clone());
    receive(link, machine, account, Purpose::Hands, path, |read| RemoteCall::ReadFile { request, read }, sender).await
}

pub async fn read_attachment(
    link: &Arc<Link>,
    machine: MachineName,
    account: AccountName,
    attachment: fastctx_types::attachments::AttachmentSource,
    range: FileRange,
    sender: &Sender,
) -> Result<FileReadResult, Failure> {
    receive(
        link,
        machine,
        account,
        Purpose::Agents,
        None,
        |read| RemoteCall::ReadAttachment { attachment, read, range },
        sender,
    )
    .await
}

/// Why a read's call failed, for whoever goes on with it: a call that may
/// not have reached the machine, or whose answer did not come, is
/// unanswered; the machine's own refusals and absence are said as they are.
fn call_failed(failure: CallFailure) -> Failure {
    match failure {
        CallFailure::NotConnected { .. }
        | CallFailure::Unknown { .. }
        | CallFailure::Unreadable { .. }
        | CallFailure::Unreachable { .. } => Failure::unanswered(failure.to_string()),
        other => Failure::new(other.to_string()),
    }
}

/// What ended a read's data connection early: the connection itself, or
/// what came on it.
enum Ended {
    Broke(String),
    Wrong(String),
}

async fn receive(
    link: &Arc<Link>,
    machine: MachineName,
    account: AccountName,
    purpose: Purpose,
    path: Option<String>,
    call: impl FnOnce(u64) -> RemoteCall,
    sender: &Sender,
) -> Result<FileReadResult, Failure> {
    let read = u64::from_le_bytes(crate::sign::random::<8>().map_err(|e| Failure::new(e.to_string()))?);
    let (complete, completed) = oneshot::channel();
    let stream_error = Arc::new(Mutex::new(None));
    let receiving_error = stream_error.clone();
    let (receiving, from, sender) = (link.clone(), machine.clone(), sender.clone());
    let feed: Feed = Box::new(move |mut stream| {
        Box::pin(async move {
            let received = async {
                let mut n = 0;
                loop {
                    let signed: Signed = stream.hear().await.map_err(Ended::Broke)?;
                    let piece: Piece =
                        receiving.unseal(&from, Domain::FileRead, &signed).await.map_err(Ended::Wrong)?;
                    if piece.read != read || piece.n != n {
                        return Err(Ended::Wrong(format!(
                            "{from} sent an out-of-order piece for a different file read"
                        )));
                    }
                    n += 1;
                    let part = match piece.part {
                        Content::Opened { info } => {
                            if !receiving.view().same(&info.machine, &from)
                                || path.as_ref().is_some_and(|path| info.path != *path)
                            {
                                return Err(Ended::Wrong(format!(
                                    "{from} named a different machine or path in the file read"
                                )));
                            }
                            Part::Opened(info)
                        }
                        Content::Chunk { offset, bytes, sha256: expected } => {
                            // The sender uses one data-message payload per block.
                            // Reject larger declarations before allocating for them.
                            if bytes == 0 || bytes > crate::wire::MAX_DATA_MESSAGE as u64 {
                                return Err(Ended::Wrong(format!("{from} declared an invalid file-read block size")));
                            }
                            let mut data = Vec::with_capacity(bytes as usize);
                            stream
                                .take(bytes, |piece| {
                                    data.extend_from_slice(piece);
                                    Ok(())
                                })
                                .await
                                .map_err(Ended::Broke)?;
                            if Digest(sha256(&data)) != expected {
                                return Err(Ended::Wrong(format!(
                                    "{from}'s file bytes do not match its signed digest"
                                )));
                            }
                            Part::Chunk { offset, bytes: data }
                        }
                        Content::Finished { result } => {
                            let _ = complete.send(result);
                            return stream.done().await.map_err(Ended::Broke);
                        }
                    };
                    // The reader let go: nothing more is wanted of this read.
                    sender.send(Ok(part)).await.map_err(|e| Ended::Broke(e.to_string()))?;
                }
            }
            .await;
            if let Err(reason) = received {
                *receiving_error.lock().expect("never poisoned") = Some(reason);
            }
            // A refusal can arrive before any signed byte header. Its
            // control reply remains authoritative; a successful reply
            // still needs a complete, verified stream below.
            Ok(())
        })
    });
    let reply = link.call_with_stream(&machine, &account, purpose, call(read), feed).await.map_err(call_failed)??;
    match stream_error.lock().expect("never poisoned").take() {
        Some(Ended::Broke(reason)) => {
            return Err(Failure::unanswered(format!("receiving the signed file read: {reason}")));
        }
        Some(Ended::Wrong(reason)) => return Err(Failure::new(format!("receiving the signed file read: {reason}"))),
        None => {}
    }
    let RemoteReply::FileRead { result } = reply else {
        return Err(Failure::new(format!("{machine} answered something other than a file read")));
    };
    // The answer came, but the data connection ended before the piece
    // that says the same.
    let terminal =
        completed.await.map_err(|_| Failure::unanswered("the file read ended without its signed final piece"))?;
    if terminal != result {
        return Err(Failure::new("the file read's final piece disagrees with its control reply"));
    }
    Ok(result)
}
