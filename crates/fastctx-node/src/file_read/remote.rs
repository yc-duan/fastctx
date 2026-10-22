//! The serving side of the shared authenticated file-read protocol.

use super::Part;
use crate::Node;
use fastctx_net::data::Stream;
use fastctx_net::file_read::{Content, Piece};
use fastctx_net::sign::{Digest, Domain, sha256};
use fastctx_types::file_read::*;
use fastctx_types::ipc::Failure;
use fastctx_types::remote::RemoteReply;
use std::sync::Arc;

pub(crate) async fn serve(
    node: &Arc<Node>,
    link: &Arc<fastctx_net::link::Link>,
    request: ReadFileRequest,
    read: u64,
    stream: Stream,
) -> Result<RemoteReply, Failure> {
    let mut parts = super::start(node.clone(), Default::default(), None, request);
    send(link, &node.machine(), &mut parts, read, stream).await
}

pub(crate) async fn serve_attachment(
    node: &Arc<Node>,
    link: &Arc<fastctx_net::link::Link>,
    attachment: fastctx_types::attachments::AttachmentSource,
    range: FileRange,
    read: u64,
    stream: Stream,
) -> Result<RemoteReply, Failure> {
    let mut parts = super::start_attachment(node.clone(), attachment, range);
    send(link, &node.machine(), &mut parts, read, stream).await
}

/// Sends the read's pieces down the caller's data connection. A connection
/// that ends on the way is a read that broke off, not an answer about the
/// file: the caller goes on from what came (`world-network.md`, 断开不是结束).
async fn send(
    link: &Arc<fastctx_net::link::Link>,
    serving: &fastctx_types::MachineName,
    parts: &mut super::Receiver,
    read: u64,
    mut stream: Stream,
) -> Result<RemoteReply, Failure> {
    let mut n = 0;
    loop {
        let part = tokio::select! {
            part = parts.recv() => part,
            () = stream.closed() => return Err(Failure::unanswered("the file read's caller disconnected")),
        };
        let Some(part) = part else { break };
        let (content, bytes, ended) = match part? {
            Part::Opened(mut info) => {
                info.machine = serving.clone();
                (Content::Opened { info }, None, None)
            }
            Part::Chunk { offset, bytes } => {
                let content = Content::Chunk { offset, bytes: bytes.len() as u64, sha256: Digest(sha256(&bytes)) };
                (content, Some(bytes), None)
            }
            Part::Finished(result) => (Content::Finished { result: result.clone() }, None, Some(result)),
        };
        let signed = link.sign(Domain::FileRead, &Piece { read, n, part: content });
        stream.say(&signed).await.map_err(Failure::unanswered)?;
        if let Some(bytes) = bytes {
            stream.send(&bytes).await.map_err(Failure::unanswered)?;
        }
        if let Some(result) = ended {
            stream.finish().await.map_err(Failure::unanswered)?;
            return Ok(RemoteReply::FileRead { result });
        }
        n += 1;
    }
    Err(Failure::new("the file reader stopped without a result"))
}
