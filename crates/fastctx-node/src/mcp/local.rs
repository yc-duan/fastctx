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

fn prepare(
    node: &Node,
    peer: &Peer,
    name: &str,
) -> Result<(Arc<fastctx_net::link::Link>, fastctx_types::AccountName, fastctx_types::MachineName), Failure> {
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
    crate::assists::permitted(node, &about, &accounts[0])?;
    let (link, account) = crate::profile::scope(node, &caller, None, None)?;
    let view = link.profile_read(&account).map_err(Failure::new)?;
    if crate::session_options::read(node, &address.resident)?.mcp_off.contains(name) {
        return Err(Failure::refused("this MCP is off in the conversation"));
    }
    let Some(Entry::Command { machine }) = view.profile.servers.get(name) else {
        return Err(Failure::refused("this is not a registered command MCP"));
    };
    let directory = crate::authorization::current_view(&link)?;
    directory
        .authorize(
            &account,
            fastctx_net::authorization::Action::Call { from: &directory.me, to: machine, purpose: Purpose::Hands },
        )
        .map_err(Failure::from)?;
    Ok((link, account, machine.clone()))
}

pub(crate) async fn serve(
    node: &Arc<Node>,
    peer: &Peer,
    name: String,
    id: u64,
    mut reader: FrameReader,
    mut writer: FrameWriter,
) -> std::io::Result<()> {
    let mut changes = node.views.subscribe();
    let mut ways = node.ways.subscribe();
    let prepared = prepare(node, peer, &name);
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
    let local = link.view().is_me(&machine);
    let server =
        if local { Some(super::store::get(node, &link.membership().id, &account, &name).map_err(io)?) } else { None };
    let revoked = async {
        loop {
            let (now_link, now_account, now_machine) = prepare(node, peer, &name).map_err(|failure| failure.message)?;
            if now_link.membership().id != link.membership().id || now_account != account || now_machine != machine {
                return Err::<(), String>("the MCP's account or destination changed".into());
            }
            if let Some(server) = &server
                && super::store::get(node, &link.membership().id, &account, &name).as_ref() != Ok(server)
            {
                return Err("the MCP was changed or removed".into());
            }
            tokio::select! {
                changed = changes.changed() => changed.map_err(|error| error.to_string())?,
                changed = ways.changed() => changed.map_err(|error| error.to_string())?,
            }
        }
    };
    prepare(node, peer, &name).map_err(io)?;
    writer.send(&serde_json::to_vec(&NodeFrame::McpOpened { id }).unwrap()).await?;
    if let Some(server) = &server {
        let mut process = super::process::start(server, &node.fastctx, &node.root).map_err(io)?;
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
    let call = link.call_with_stream(
        &machine,
        &account,
        Purpose::Hands,
        fastctx_types::remote::RemoteCall::McpServe { name: name.clone() },
        feed,
    );
    tokio::select! {
        result = call => { result.map_err(io)?.map_err(io)?; Ok(()) },
        result = revoked => result.map_err(io),
    }
}
