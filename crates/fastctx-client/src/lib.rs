//! The one way into the node.
//!
//! Commands, the TUI, a job's supervisor and later the graphical interface
//! all reach the node through a `Client`: it connects to the node's local
//! endpoint, says who is speaking, and sends calls, each answered once.
//! Nothing outside the node writes the World's state; clients ask.
//!
//! Above it, what every interface shares: [`World`], one method for each
//! thing asked of the World; [`TownView`], who is where in the shape to show
//! it; [`Conversation`], talking to a resident and following its turns.

pub mod conversation;
pub mod shown;
pub mod town;
pub mod world;

pub use conversation::{Conversation, Following, Moment};
pub use town::TownView;
pub use world::{Error, Said, TownRecord, World};

use fastctx_platform::ipc::{self, Connection, Endpoint};
use fastctx_types::ipc::{ClientFrame, Failure, Hello, NodeFrame, PROTOCOL, Peer, SessionHint, Welcome};
use fastctx_types::layout::WorldDir;
use fastctx_types::session::Watched;
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
    #[error("the node was started but exited before it was ready; its log is {log}")]
    NodeDidNotStart { log: String },
    #[error(
        "the node is not running ({endpoint}); a system service keeps it and brings it back, \
         and `fastctx service node status` says how it stands"
    )]
    KeptByService { endpoint: String },
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
        let endpoint = match Endpoint::for_world(world.path()) {
            Ok(endpoint) => endpoint,
            // The World's directory is made by its first node; without it
            // there has never been a node here.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(ConnectError::NoNode { endpoint: world.path().display().to_string() });
            }
            Err(error) => return Err(ConnectError::Io { endpoint: world.path().display().to_string(), error }),
        };
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

    /// Connects to the node, first starting it if it is not running. The
    /// node is started on its own, with the user's environment, so that it
    /// outlives whatever started it and carries nothing of that process.
    /// A node a system service keeps is started by nobody else: a second
    /// one would hold the World while the service's waits for it.
    pub async fn connect_or_start(world: &WorldDir, peer: Peer) -> Result<Self, ConnectError> {
        match Self::connect(world, peer.clone()).await {
            Err(ConnectError::NoNode { endpoint }) if world.node_service().exists() => {
                Err(ConnectError::KeptByService { endpoint })
            }
            Err(ConnectError::NoNode { endpoint }) => {
                let started = start_node().map_err(|error| ConnectError::Io { endpoint: endpoint.clone(), error })?;
                loop {
                    match Self::connect(world, peer.clone()).await {
                        Err(ConnectError::NoNode { .. }) => {}
                        connected => return connected,
                    }
                    // Another client may have started a node first, in which
                    // case this one exits at once; only when no node listens
                    // and the one started here is gone has the start failed.
                    let alive = match &started.identity {
                        Some(identity) => fastctx_platform::is_alive(identity).unwrap_or(true),
                        None => false,
                    };
                    if !alive {
                        return match Self::connect(world, peer.clone()).await {
                            Err(ConnectError::NoNode { .. }) => {
                                Err(ConnectError::NodeDidNotStart { log: world.node_log().display().to_string() })
                            }
                            connected => connected,
                        };
                    }
                    // A node takes a moment to open its endpoint after it
                    // starts; asking again soon costs nothing.
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                }
            }
            connected => connected,
        }
    }

    /// Waits until the node closes this connection, as when it stops. Kept
    /// open, a connection a session arrived on is how the node knows the
    /// resident is here.
    pub async fn closed(mut self) {
        while let Ok(Some(_)) = self.connection.recv().await {}
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
            NodeFrame::Reply { id: answered, .. } => {
                Err(broken(format!("the node answered call {answered}, not {id}")))
            }
            NodeFrame::Watched { .. } => Err(broken("the node answered a call as a watch".to_string())),
        }
    }

    /// Sends a `Watch` call; the connection carries the watch from then on.
    pub async fn watch(mut self, call: Call) -> Result<Watching, CallError> {
        self.next += 1;
        let id = self.next;
        self.connection.send(&to_json(&ClientFrame::Call { id, call })).await.map_err(CallError::Broken)?;
        Ok(Watching { client: self, id })
    }
}

/// A watch the node hands over as it goes; dropping it ends the watch.
pub struct Watching {
    client: Client,
    id: u64,
}

impl Watching {
    /// The next piece; `None` once the watch ended as it should.
    pub async fn next(&mut self) -> Result<Option<Watched>, CallError> {
        let frame = self
            .client
            .connection
            .recv()
            .await
            .map_err(CallError::Broken)?
            .ok_or_else(|| CallError::Broken(std::io::ErrorKind::UnexpectedEof.into()))?;
        let broken = |what: String| CallError::Broken(std::io::Error::other(what));
        match serde_json::from_slice::<NodeFrame>(&frame).map_err(|error| broken(error.to_string()))? {
            NodeFrame::Watched { id, watched } if id == self.id => Ok(Some(watched)),
            NodeFrame::Reply { id, reply } if id == self.id => reply.map(|_| None).map_err(CallError::Failed),
            _ => Err(broken("the node answered another call on a watch".to_string())),
        }
    }
}

/// Who a command speaks as, from its environment: a resident, when the
/// World's `run` started it; an agent's session, when that agent's own
/// shell did, as its host says; otherwise whoever is at the terminal.
pub fn command_peer() -> Peer {
    let resident = std::env::var("FASTCTX_RESIDENT").ok().and_then(|address| address.parse().ok());
    let set = |name: &str| std::env::var(name).ok().filter(|value| !value.is_empty());
    let session = match (set("CODEX_THREAD_ID"), set("CLAUDE_CODE_MESSAGING_SOCKET")) {
        (Some(thread), _) => Some(SessionHint::Codex { thread }),
        (None, Some(inbox)) => Some(SessionHint::Claude { inbox }),
        (None, None) => None,
    };
    Peer::Command { resident, session }
}

/// Starts this binary's node on its own.
fn start_node() -> std::io::Result<fastctx_platform::DetachedProcess> {
    let executable = std::env::current_exe()?;
    let home = fastctx_platform::user_home()?;
    let environment = fastctx_platform::world_process_environment()?;
    fastctx_platform::spawn_detached(&executable, &[std::ffi::OsStr::new("node")], &home, &environment)
}

fn to_json(value: &impl serde::Serialize) -> Vec<u8> {
    serde_json::to_vec(value).expect("the World's types always serialize")
}
