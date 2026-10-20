//! One JSON-RPC conversation on a full-duplex World data connection.

use super::process::Process;
use fastctx_net::data::{ReadHalf, Stream, WriteHalf};
use fastctx_types::mcp_message;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct Header {
    line: usize,
}

pub(super) async fn receive(reader: &mut ReadHalf) -> Result<Vec<u8>, String> {
    let Header { line } = reader.hear().await?;
    if line > mcp_message::MAX_LINE {
        return Err("an MCP message exceeds 32 MiB".into());
    }
    let mut bytes = Vec::with_capacity(line);
    reader
        .take(line as u64, |part| {
            bytes.extend_from_slice(part);
            Ok(())
        })
        .await?;
    mcp_message::parse(&bytes)?;
    Ok(bytes)
}

pub(super) async fn send(writer: &mut WriteHalf, bytes: &[u8]) -> Result<(), String> {
    mcp_message::parse(bytes)?;
    writer.say(&Header { line: bytes.len() }).await?;
    writer.send(bytes).await
}

pub(super) async fn serve(process: &mut Process, stream: Stream) -> Result<(), String> {
    let (mut reader, mut writer) = stream.split();
    let to_program = async {
        loop {
            let line = receive(&mut reader).await?;
            process.input.send(line).await.map_err(|_| "the MCP program stopped reading".to_string())?;
        }
        #[allow(unreachable_code)]
        Ok::<(), String>(())
    };
    let from_program = async {
        while let Some(line) = process.output.recv().await {
            send(&mut writer, &line?).await?;
        }
        Err::<(), String>("the MCP program stopped".into())
    };
    tokio::select! { result = to_program => result, result = from_program => result }
}
