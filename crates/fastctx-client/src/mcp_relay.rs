//! A dedicated local JSON-RPC connection. The caller owns reconnection and IDs.

use crate::{CallError, Client};
use fastctx_platform::ipc::{FrameReader, FrameWriter};
use fastctx_types::{
    Call,
    ipc::{ClientFrame, Failure, NodeFrame},
};

impl Client {
    pub async fn mcp_relay(mut self, name: String) -> Result<(McpReader, McpWriter), CallError> {
        if !self.profiles {
            return Err(CallError::Failed(Failure::not_run(format!("upgrade {} to use MCP relay", self.machine))));
        }
        self.next += 1;
        let id = self.next;
        self.connection
            .send(&crate::to_json(&ClientFrame::Call { id, call: Call::McpRelay { name } }))
            .await
            .map_err(CallError::Broken)?;
        match self.frame().await? {
            NodeFrame::McpOpened { id: opened } if opened == id => {}
            NodeFrame::Reply { reply: Err(error), .. } => return Err(CallError::Failed(error)),
            _ => return Err(CallError::Broken(std::io::Error::other("the node did not open MCP relay"))),
        }
        let (reader, writer) = self.connection.split();
        Ok((McpReader { reader, id }, McpWriter(writer)))
    }
}
pub struct McpReader {
    reader: FrameReader,
    id: u64,
}
pub struct McpWriter(FrameWriter);
impl McpReader {
    pub async fn receive(&mut self) -> Result<Vec<u8>, String> {
        let bytes =
            self.reader.recv_up_to(256).await.map_err(|e| e.to_string())?.ok_or("the MCP relay connection ended")?;
        let NodeFrame::McpLine { id, line } = serde_json::from_slice(&bytes).map_err(|e| e.to_string())? else {
            return Err("expected an MCP line header".into());
        };
        if id != self.id || line > fastctx_types::mcp_message::MAX_LINE {
            return Err("invalid MCP line header".into());
        }
        let bytes =
            self.reader.recv_up_to(line).await.map_err(|e| e.to_string())?.ok_or("the MCP line did not arrive")?;
        if bytes.len() != line {
            return Err("the MCP line length changed".into());
        }
        fastctx_types::mcp_message::parse(&bytes)?;
        Ok(bytes)
    }
}
impl McpWriter {
    pub async fn send(&mut self, bytes: &[u8]) -> Result<(), String> {
        fastctx_types::mcp_message::parse(bytes)?;
        self.0.send(&crate::to_json(&ClientFrame::McpLine { line: bytes.len() })).await.map_err(|e| e.to_string())?;
        self.0.send(bytes).await.map_err(|e| e.to_string())
    }
}
