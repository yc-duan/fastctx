//! The one way into the node.
//!
//! Commands, the TUI, a job's supervisor and later the graphical interface
//! all reach the node through a `Client`: it connects to the node's local
//! endpoint, says who is speaking, and sends calls, each answered once.
//! Nothing outside the node writes the World's state; clients ask.

use fastctx_platform::ipc::{self, Connection, Endpoint};
use fastctx_types::ipc::{ClientFrame, Failure, Hello, NodeFrame, PROTOCOL, Peer, Welcome};
use fastctx_types::layout::WorldDir;
use fastctx_types::{Call, MachineName, Reply};

/// Why there is no connection to the node.
#[derive(Debug, thiserror::Error)]
pub enum ConnectError {
    #[error("the node is not running ({endpoint})")]
    NoNode { endpoint: String },
    #[error("the node refused: {reason}")]
    Refused { reason: String },
    #[error("talking to the node at {endpoint}: {error}")]
    Io { endpoint: String, error: std::io::Error },
}

/// A failed call: the node said no, or the connection broke on the way.
#[derive(Debug, thiserror::Error)]
pub enum CallError {
    #[error("{0}")]
    Failed(Failure),
    /// The connection broke with the call in flight. Whether the node did
    /// what was asked is not known; the call is not sent again.
    #[error("the connection to the node broke before it answered; whether it acted is not known: {0}")]
    Broken(std::io::Error),
}

pub struct Client {
    connection: Connection,
    next: u64,
    /// The machine the node runs on.
    pub machine: MachineName,
    /// The node binary's version.
    pub node_version: String,
}

impl Client {
    /// Connects to the node of the World in `world` and introduces `peer`.
    pub async fn connect(world: &WorldDir, peer: Peer) -> Result<Self, ConnectError> {
        let endpoint = Endpoint::for_world(world.path())
            .map_err(|error| ConnectError::Io { endpoint: world.path().display().to_string(), error })?;
        let io_error = |error: std::io::Error| ConnectError::Io { endpoint: endpoint.to_string(), error };
        let mut connection = match ipc::connect(&endpoint).await {
            Ok(connection) => connection,
            Err(error)
                if matches!(error.kind(), std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused) =>
            {
                return Err(ConnectError::NoNode { endpoint: endpoint.to_string() });
            }
            Err(error) => return Err(io_error(error)),
        };
        let hello = Hello { protocol: PROTOCOL, version: env!("CARGO_PKG_VERSION").to_string(), peer };
        connection.send(&to_json(&hello)).await.map_err(io_error)?;
        let welcome = connection
            .recv()
            .await
            .map_err(io_error)?
            .ok_or_else(|| io_error(std::io::ErrorKind::UnexpectedEof.into()))?;
        match serde_json::from_slice::<Welcome>(&welcome).map_err(|error| io_error(std::io::Error::other(error)))? {
            Welcome::Accepted { machine, version } => Ok(Self { connection, next: 0, machine, node_version: version }),
            Welcome::Refused { reason } => Err(ConnectError::Refused { reason }),
        }
    }

    /// Sends `call` and waits for its answer.
    pub async fn call(&mut self, call: Call) -> Result<Reply, CallError> {
        self.next += 1;
        let id = self.next;
        self.connection.send(&to_json(&ClientFrame::Call { id, call })).await.map_err(CallError::Broken)?;
        let frame = self
            .connection
            .recv()
            .await
            .map_err(CallError::Broken)?
            .ok_or_else(|| CallError::Broken(std::io::ErrorKind::UnexpectedEof.into()))?;
        let broken = |what: String| CallError::Broken(std::io::Error::other(what));
        match serde_json::from_slice::<NodeFrame>(&frame).map_err(|error| broken(error.to_string()))? {
            NodeFrame::Reply { id: answered, reply } if answered == id => reply.map_err(CallError::Failed),
            // Calls on one client go one at a time, so any other id is a
            // fault in the node.
            NodeFrame::Reply { id: answered, .. } => Err(broken(format!("the node answered call {answered}, not {id}"))),
        }
    }
}

fn to_json(value: &impl serde::Serialize) -> Vec<u8> {
    serde_json::to_vec(value).expect("the World's types always serialize")
}
