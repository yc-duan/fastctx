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
    mut stream: Stream,
) -> Result<RemoteReply, Failure> {
    let mut parts = super::start(node.clone(), Default::default(), None, request);
    let mut n = 0;
    while let Some(part) = parts.recv().await {
        let (content, bytes, ended) = match part? {
            Part::Opened(info) => (Content::Opened { info }, None, None),
            Part::Chunk { offset, bytes } => {
                let content = Content::Chunk { offset, bytes: bytes.len() as u64, sha256: Digest(sha256(&bytes)) };
                (content, Some(bytes), None)
            }
            Part::Finished(result) => (Content::Finished { result: result.clone() }, None, Some(result)),
        };
        let signed = link.sign(Domain::FileRead, &Piece { read, n, part: content });
        stream.say(&signed).await.map_err(Failure::new)?;
        if let Some(bytes) = bytes {
            stream.send(&bytes).await.map_err(Failure::new)?;
        }
        if let Some(result) = ended {
            stream.finish().await.map_err(Failure::new)?;
            return Ok(RemoteReply::FileRead { result });
        }
        n += 1;
    }
    Err(Failure::new("the file reader stopped without a result"))
}
