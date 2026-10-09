//! The signed original-byte read shared by machine and device clients.

use crate::link::{Feed, Link};
use crate::sign::{Digest, Domain, Signed, sha256};
use fastctx_types::access::Purpose;
use fastctx_types::{AccountName, MachineName};
use fastctx_types::file_read::*;
use fastctx_types::ipc::Failure;
use fastctx_types::remote::{RemoteCall, RemoteReply};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
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
    let read = u64::from_le_bytes(crate::sign::random::<8>().map_err(|e| Failure::new(e.to_string()))?);
    let (complete, completed) = oneshot::channel();
    let (receiving, from, sender) = (link.clone(), machine.clone(), sender.clone());
    let path = request.path.clone();
    let feed: Feed = Box::new(move |mut stream| {
        Box::pin(async move {
            let mut n = 0;
            loop {
                let signed: Signed = stream.hear().await?;
                let piece: Piece = receiving.unseal(&from, Domain::FileRead, &signed).await?;
                if piece.read != read || piece.n != n {
                    return Err(format!("{from} sent an out-of-order piece for a different file read"));
                }
                n += 1;
                let part = match piece.part {
                    Content::Opened { info } => {
                        if info.machine != from || info.path != path {
                            return Err(format!("{from} named a different machine or path in the file read"));
                        }
                        Part::Opened(info)
                    }
                    Content::Chunk { offset, bytes, sha256: expected } => {
                        // The sender uses one data-message payload per block.
                        // Reject larger declarations before allocating for them.
                        if bytes == 0 || bytes > crate::wire::MAX_DATA_MESSAGE as u64 {
                            return Err(format!("{from} declared an invalid file-read block size"));
                        }
                        let mut data = Vec::with_capacity(bytes as usize);
                        stream
                            .take(bytes, |piece| {
                                data.extend_from_slice(piece);
                                Ok(())
                            })
                            .await?;
                        if Digest(sha256(&data)) != expected {
                            return Err(format!("{from}'s file bytes do not match its signed digest"));
                        }
                        Part::Chunk { offset, bytes: data }
                    }
                    Content::Finished { result } => {
                        let _ = complete.send(result);
                        return stream.done().await;
                    }
                };
                sender.send(Ok(part)).await.map_err(|e| e.to_string())?;
            }
        })
    });
    let reply = link
        .call_with_stream(&machine, &account, Purpose::Hands, RemoteCall::ReadFile { request, read }, feed)
        .await
        .map_err(|e| Failure::new(e.to_string()))??;
    let RemoteReply::FileRead { result } = reply else {
        return Err(Failure::new(format!("{machine} answered something other than a file read")));
    };
    let terminal = completed.await.map_err(|_| Failure::new("the file read ended without its signed final piece"))?;
    if terminal != result {
        return Err(Failure::new("the file read's final piece disagrees with its control reply"));
    }
    Ok(result)
}
