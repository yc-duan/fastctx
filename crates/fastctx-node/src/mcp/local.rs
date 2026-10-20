//! The resident's local relay connection, admitted again on each reconnect.

use crate::{
    Node,
    town::caller::{self, Caller},
};
use fastctx_platform::ipc::{FrameReader, FrameWriter};
use fastctx_types::{
    access::{Acting, Purpose},
    ipc::{ClientFrame, Failure, NodeFrame, Peer},
    profile::Entry,
};
use std::sync::{Arc, Mutex};

fn io(error: impl ToString) -> std::io::Error {
    std::io::Error::other(error.to_string())
}

async fn read(reader: &mut FrameReader) -> Result<Vec<u8>, String> {
    let head = reader.recv_up_to(256).await.map_err(|e| e.to_string())?.ok_or("the MCP client closed its input")?;
    let ClientFrame::McpLine { line } = serde_json::from_slice(&head).map_err(|e| e.to_string())? else {
        return Err("expected an MCP line header".into());
    };
    if line > fastctx_types::mcp_message::MAX_LINE {
        return Err("an MCP message exceeds 32 MiB".into());
    }
    let bytes =
        reader.recv_up_to(line).await.map_err(|e| e.to_string())?.ok_or("the MCP client left before its message")?;
    if bytes.len() != line {
        return Err("the MCP message length does not match its header".into());
    }
    fastctx_types::mcp_message::parse(&bytes)?;
    Ok(bytes)
}
async fn write(writer: &mut FrameWriter, id: u64, bytes: &[u8]) -> Result<(), String> {
    fastctx_types::mcp_message::parse(bytes)?;
    writer
        .send(&serde_json::to_vec(&NodeFrame::McpLine { id, line: bytes.len() }).unwrap())
        .await
        .map_err(|e| e.to_string())?;
    writer.send(bytes).await.map_err(|e| e.to_string())
}

pub(crate) async fn serve(
    node: &Arc<Node>,
    peer: &Peer,
    name: String,
    id: u64,
    mut reader: FrameReader,
    mut writer: FrameWriter,
) -> std::io::Result<()> {
    let prepared = (|| {
        let caller = caller::of(node, peer)?;
        let Caller::Resident(address) = &caller else {
            return Err(Failure::refused("an MCP relay belongs to an account conversation"));
        };
        if !node.is_me(&address.machine) {
            return Err(Failure::refused("start the relay where its conversation runs"));
        }
        let (about, _) = node
            .residents
            .about(&address.resident)
            .ok_or_else(|| Failure::refused("the relay's conversation is unknown"))?;
        let Acting::Accounts { accounts } = &about.acting else {
            return Err(Failure::refused("the conversation works for no account"));
        };
        if accounts.len() != 1 {
            return Err(Failure::refused("the conversation works for more than one account"));
        }
        let (link, account) = crate::profile::scope(node, &caller, None, None)?;
        let view = link.profile_read(&account).map_err(Failure::new)?;
        if crate::session_options::read(node, &address.resident)?.mcp_off.contains(&name) {
            return Err(Failure::refused("this MCP is off in the conversation"));
        }
        let Some(Entry::Command { machine }) = view.profile.servers.get(&name) else {
            return Err(Failure::refused("this is not a registered command MCP"));
        };
        link.view().admits(&link.view().me, &account, machine, Purpose::Hands).map_err(Failure::refused)?;
        Ok((link, account, machine.clone()))
    })();
    let (link, account, machine) = match prepared {
        Ok(prepared) => prepared,
        Err(failure) => {
            writer.send(&serde_json::to_vec(&NodeFrame::Reply { id, reply: Err(failure) }).unwrap()).await?;
            return writer.drain().await;
        }
    };
    if let Err(failure) = super::supports(node, &link, &account, &machine).await {
        writer.send(&serde_json::to_vec(&NodeFrame::Reply { id, reply: Err(failure) }).unwrap()).await?;
        return writer.drain().await;
    }
    writer.send(&serde_json::to_vec(&NodeFrame::McpOpened { id }).unwrap()).await?;
    if link.view().is_me(&machine) {
        let server = super::store::get(node, &link.membership().id, &account, &name).map_err(io)?;
        let mut process = super::process::start(&server, &node.fastctx, &node.root).map_err(io)?;
        let mut changes = node.views.subscribe();
        let revoked = async {
            loop {
                changes.changed().await.map_err(|e| e.to_string())?;
                if super::store::get(node, &link.membership().id, &account, &name).as_ref() != Ok(&server) {
                    return Err::<(), String>("the MCP was changed or removed".into());
                }
            }
        };
        let incoming = async {
            loop {
                process
                    .input
                    .send(read(&mut reader).await?)
                    .await
                    .map_err(|_| "the MCP stopped reading".to_string())?;
            }
            #[allow(unreachable_code)]
            Ok::<(), String>(())
        };
        let outgoing = async {
            while let Some(line) = process.output.recv().await {
                write(&mut writer, id, &line?).await?;
            }
            Err::<(), String>("the MCP stopped".into())
        };
        let result = tokio::select! { result=incoming=>result, result=outgoing=>result, result=revoked=>result };
        let log = process.log.lock().expect("never poisoned").clone();
        if let Err(error) = super::store::keep_log(node, &link.membership().id, &account, &name, &log) {
            node.log.line(format_args!("keeping MCP diagnostics: {error}"));
        }
        return result.map_err(io);
    }
    // The pair is consumed only by the one admitted call. Retrying a failed
    // connection belongs to the relay process, which settles pending IDs.
    let pair = Arc::new(Mutex::new(Some((reader, writer))));
    let feed: fastctx_net::link::Feed = Box::new(move |stream| {
        let (mut reader, mut writer) = pair.lock().expect("never poisoned").take().expect("one data connection");
        Box::pin(async move {
            let (mut receive, mut send) = stream.split();
            let incoming = async {
                loop {
                    super::stream::send(&mut send, &read(&mut reader).await?).await?;
                }
                #[allow(unreachable_code)]
                Ok::<(), String>(())
            };
            let outgoing = async {
                loop {
                    write(&mut writer, id, &super::stream::receive(&mut receive).await?).await?;
                }
                #[allow(unreachable_code)]
                Ok::<(), String>(())
            };
            tokio::select! { result=incoming=>result, result=outgoing=>result }
        })
    });
    link.call_with_stream(
        &machine,
        &account,
        Purpose::Hands,
        fastctx_types::remote::RemoteCall::McpServe { name },
        feed,
    )
    .await
    .map_err(io)?
    .map_err(io)?;
    Ok(())
}
