//! One client's connection: the welcome, then its calls one at a time.

use std::sync::Arc;

use fastctx_platform::ipc::Connection;
use fastctx_types::ipc::{ClientFrame, Failure, Hello, NodeFrame, PROTOCOL, Peer, Welcome};
use fastctx_types::{Call, Reply};

use crate::Node;

pub async fn serve(node: Arc<Node>, connection: Connection) {
    if let Err(error) = converse(&node, connection).await {
        node.log.line(format_args!("a connection ended in error: {error}"));
    }
}

async fn converse(node: &Node, connection: Connection) -> std::io::Result<()> {
    let (mut reader, mut writer) = connection.split();
    let Some(hello) = reader.recv().await? else { return Ok(()) };
    let hello: Hello = parse(&hello)?;
    if hello.protocol != PROTOCOL {
        let reason = format!(
            "this node is fastctx {} and speaks protocol {PROTOCOL}; the client is fastctx {} and speaks protocol {}",
            env!("CARGO_PKG_VERSION"),
            hello.version,
            hello.protocol
        );
        writer.send(&json(&Welcome::Refused { reason })).await?;
        return Ok(());
    }
    let welcome = Welcome::Accepted { machine: node.machine.clone(), version: env!("CARGO_PKG_VERSION").to_string() };
    writer.send(&json(&welcome)).await?;
    while let Some(frame) = reader.recv().await? {
        let ClientFrame::Call { id, call } = parse(&frame)?;
        let reply = handle(node, &hello.peer, call).await;
        writer.send(&json(&NodeFrame::Reply { id, reply })).await?;
    }
    Ok(())
}

async fn handle(node: &Node, _peer: &Peer, call: Call) -> Result<Reply, Failure> {
    match call {
        Call::Arrive { host, hints } => node.residents.arrive(&node.world, &node.machine, host, hints).map(Reply::Arrived),
        Call::Run { resident, town, request } => {
            node.jobs.run(&node.world, &node.machine, resident, town, request).await.map(Reply::Run)
        }
        Call::JobChanged { job, resident, town } => {
            node.jobs.changed(&node.world, job, resident, town).await.map(|()| Reply::Done)
        }
        Call::Saw { resident, town, seen } => {
            let world = node.world.clone();
            tokio::task::spawn_blocking(move || crate::residents::record_seen(&world, &resident, town.as_ref(), &seen))
                .await
                .map_err(|error| Failure::new(format!("recording what was seen: {error}")))?
                .map(|()| Reply::Done)
                .map_err(|error| Failure::new(format!("recording what was seen: {error}")))
        }
        Call::Collect { resident, town } => {
            let world = node.world.clone();
            tokio::task::spawn_blocking(move || crate::inbox::collect(&world, &resident, town.as_ref()))
                .await
                .map_err(|error| Failure::new(format!("reading the inbox: {error}")))?
                .map(|notices| Reply::Notices { notices })
                .map_err(|error| Failure::new(format!("reading the inbox: {error}")))
        }
    }
}

fn parse<T: serde::de::DeserializeOwned>(frame: &[u8]) -> std::io::Result<T> {
    serde_json::from_slice(frame).map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
}

fn json(value: &impl serde::Serialize) -> Vec<u8> {
    serde_json::to_vec(value).expect("the World's types always serialize")
}
