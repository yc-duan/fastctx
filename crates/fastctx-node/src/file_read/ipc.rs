//! Headers and original binary payloads on one exclusive local connection.

use fastctx_types::access::Written;
use std::sync::Arc;

use fastctx_platform::ipc::{FrameReader, FrameWriter};
use fastctx_types::Reply;
use fastctx_types::file_read::{FileReadPart, ReadFileRequest};
use fastctx_types::ipc::{Failure, NodeFrame};

use super::Part;
use crate::Node;

pub(crate) async fn serve(
    node: &Arc<Node>,
    acting: fastctx_types::access::Acting,
    machine: Option<Written>,
    request: ReadFileRequest,
    id: u64,
    reader: &mut FrameReader,
    writer: &mut FrameWriter,
) -> std::io::Result<()> {
    let mut parts = super::start(node.clone(), acting, machine, request);
    send(&mut parts, id, reader, writer).await
}

pub(crate) async fn serve_attachment(
    node: &Arc<Node>,
    attachment: fastctx_types::attachments::AttachmentSource,
    range: fastctx_types::file_read::FileRange,
    id: u64,
    reader: &mut FrameReader,
    writer: &mut FrameWriter,
) -> std::io::Result<()> {
    let mut parts = super::start_attachment(node.clone(), attachment, range);
    send(&mut parts, id, reader, writer).await
}

async fn send(
    parts: &mut super::Receiver,
    id: u64,
    reader: &mut FrameReader,
    writer: &mut FrameWriter,
) -> std::io::Result<()> {
    let reply = loop {
        let part = tokio::select! {
            part = parts.recv() => part,
            _ = reader.recv() => return Ok(()),
        };
        let part = match part {
            Some(Ok(Part::Finished(result))) => break Ok(Reply::FileRead { result }),
            Some(Err(failure)) => break Err(failure),
            None => break Err(Failure::new("the file reader stopped without a result")),
            Some(Ok(part)) => part,
        };
        let (part, bytes) = match part {
            Part::Opened(info) => (FileReadPart::Opened { info }, None),
            Part::Chunk { offset, bytes } => (FileReadPart::Chunk { offset, bytes: bytes.len() as u64 }, Some(bytes)),
            Part::Finished(_) => unreachable!("handled above"),
        };
        writer.send(&json(&NodeFrame::FileRead { id, part })).await?;
        if let Some(bytes) = bytes {
            writer.send(&bytes).await?;
        }
    };
    writer.send(&json(&NodeFrame::Reply { id, reply })).await?;
    writer.drain().await
}

fn json(value: &impl serde::Serialize) -> Vec<u8> {
    serde_json::to_vec(value).expect("the World's types always serialize")
}
