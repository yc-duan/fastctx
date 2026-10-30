//! The local maintenance envelope and the one shipped legacy migration.
//! Business calls are never retried under a guessed protocol version.

use std::io;

use fastctx_platform::ipc::{self, Connection, Endpoint};
use fastctx_types::ipc::{Hello, Peer, Welcome};
use fastctx_types::layout::WorldDir;
use fastctx_types::renewal::{Action, MAINTENANCE, Mismatch, Request, Response, Running};

#[derive(Debug)]
pub enum Probe {
    Running(Running),
    Legacy { mismatch: Mismatch, process: Option<fastctx_types::ProcessIdentity> },
    Absent,
}

pub async fn image(world: &WorldDir) -> io::Result<Option<std::path::PathBuf>> {
    match probe(world).await? {
        Probe::Absent => Ok(None),
        Probe::Running(node) => Ok(Some(node.executable.into())),
        Probe::Legacy { mismatch, .. } if matches!(mismatch.node_protocol, 5 | 6) => {
            // Shipped nodes publish their pinned executable in this World-
            // owned plugin manifest on every startup. The installed name may
            // already have changed; copying it would lose the rollback image.
            let path = world.path().join("claude-plugins/fastctx-world/.claude-plugin/plugin.json");
            let manifest: serde_json::Value =
                serde_json::from_slice(&std::fs::read(path)?).map_err(io::Error::other)?;
            let command = manifest
                .pointer("/mcpServers/fastctx/command")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| io::Error::other("legacy node did not publish its pinned executable"))?;
            let command = std::fs::canonicalize(command)?;
            if !command.starts_with(std::fs::canonicalize(world.path().join("bin"))?) {
                return Err(io::Error::other("legacy executable is outside this node's pinned images"));
            }
            Ok(Some(command))
        }
        Probe::Legacy { .. } => Err(io::Error::new(io::ErrorKind::Unsupported, "unknown legacy executable layout")),
    }
}

pub async fn process(world: &WorldDir) -> io::Result<Option<fastctx_types::ProcessIdentity>> {
    match connect(world).await {
        Ok(connection) => Ok(connection.server_process().cloned()),
        // Gone, or closing the connection as it was made: none serves now.
        Err(error) if absent(&error) || broken_pipe(&error) => Ok(None),
        Err(error) => Err(error),
    }
}

pub async fn probe(world: &WorldDir) -> io::Result<Probe> {
    match exchange(world, Action::Status).await {
        Ok(Response::Status { node }) => Ok(Probe::Running(node)),
        Ok(other) => Err(io::Error::other(format!("unexpected maintenance status: {other:?}"))),
        Err(error) if absent(&error) => Ok(Probe::Absent),
        // Shipped nodes close a connection whose first frame is not Hello.
        // Other errors, including permission failures, are not compatibility.
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof || broken_pipe(&error) => legacy(world).await,
        Err(error) => Err(error),
    }
}

pub async fn request(world: &WorldDir, action: Action) -> io::Result<()> {
    match probe(world).await? {
        Probe::Absent => Err(io::Error::new(io::ErrorKind::NotFound, "no local node")),
        Probe::Running(_) => match exchange(world, action).await? {
            Response::Done => Ok(()),
            Response::Refused { reason } => Err(io::Error::other(reason)),
            other => Err(io::Error::other(format!("unexpected maintenance response: {other:?}"))),
        },
        Probe::Legacy { mismatch, .. } => {
            if !matches!(action, Action::Yield) || !matches!(mismatch.node_protocol, 5 | 6) {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "this legacy node cannot do that maintenance action",
                ));
            }
            let mut connection = connect(world).await?;
            let hello = hello(mismatch.node_protocol);
            send(&mut connection, &hello).await?;
            let welcome: Welcome = receive(&mut connection).await?;
            if !matches!(welcome, Welcome::Accepted { .. }) {
                return Err(io::Error::other("legacy node refused its own protocol"));
            }
            send(&mut connection, &serde_json::json!({"kind":"call","id":1,"call":{"call":"hand_over"}})).await?;
            let answer: serde_json::Value = receive(&mut connection).await?;
            if answer != serde_json::json!({"kind":"reply","id":1,"reply":{"Ok":{"reply":"done"}}}) {
                return Err(io::Error::other(format!("legacy handover was not acknowledged: {answer}")));
            }
            Ok(())
        }
    }
}

pub async fn exchange(world: &WorldDir, action: Action) -> io::Result<Response> {
    let mut connection = connect(world).await?;
    send(&mut connection, &Request { maintenance: MAINTENANCE, action }).await?;
    receive(&mut connection).await
}

async fn legacy(world: &WorldDir) -> io::Result<Probe> {
    let mut connection = connect(world).await?;
    let process = connection.server_process().cloned();
    // Zero was never a World business protocol. Asking with it obtains the
    // old node's actual protocol without trying a sequence of versions.
    send(&mut connection, &hello(0)).await?;
    let welcome: Welcome = receive(&mut connection).await?;
    match welcome {
        Welcome::Refused { reason, mismatch } => {
            let mut mismatch = mismatch
                .or_else(|| legacy_mismatch(&reason))
                .ok_or_else(|| io::Error::other(format!("unrecognized node refusal: {reason}")))?;
            mismatch.client_protocol = fastctx_types::ipc::PROTOCOL;
            mismatch.client_version = env!("CARGO_PKG_VERSION").into();
            Ok(Probe::Legacy { mismatch, process })
        }
        _ => Err(io::Error::other("node unexpectedly accepted protocol zero")),
    }
}

/// The exact refusal grammar of the shipped 1.0.0 builds. An unknown
/// refusal remains an error; it is never classified by matching keywords.
pub fn legacy_mismatch(reason: &str) -> Option<Mismatch> {
    let rest = reason.strip_prefix("this node is fastctx ")?;
    let (node_version, rest) = rest.split_once(" and speaks protocol ")?;
    let (node_protocol, rest) = rest.split_once("; the client is fastctx ")?;
    let (client_version, client_protocol) = rest.split_once(" and speaks protocol ")?;
    Some(Mismatch {
        node_version: node_version.into(),
        node_protocol: node_protocol.parse().ok()?,
        client_version: client_version.into(),
        client_protocol: client_protocol.parse().ok()?,
    })
}

fn hello(protocol: u32) -> Hello {
    Hello {
        protocol,
        version: env!("CARGO_PKG_VERSION").into(),
        peer: Peer::Command { resident: None, session: None },
        notices: fastctx_types::ipc::NOTICES,
        shapes: fastctx_types::ipc::SHAPES,
    }
}

async fn connect(world: &WorldDir) -> io::Result<Connection> {
    ipc::connect(&Endpoint::for_world(world.path())?).await
}

async fn send(connection: &mut Connection, value: &impl serde::Serialize) -> io::Result<()> {
    connection.send(&serde_json::to_vec(value).map_err(io::Error::other)?).await
}

async fn receive<T: serde::de::DeserializeOwned>(connection: &mut Connection) -> io::Result<T> {
    let frame = connection.recv().await?.ok_or_else(|| io::Error::from(io::ErrorKind::UnexpectedEof))?;
    serde_json::from_slice(&frame).map_err(io::Error::other)
}

fn absent(error: &io::Error) -> bool {
    matches!(error.kind(), io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused)
}

fn broken_pipe(error: &io::Error) -> bool {
    matches!(error.kind(), io::ErrorKind::BrokenPipe | io::ErrorKind::ConnectionReset)
}
