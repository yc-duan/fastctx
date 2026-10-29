//! The interface to the World, through a local node or a device's own link.
//!
//! Commands, the TUI, a job's supervisor and later the graphical interface
//! reach a local node through a `Client`: it connects to the node's local
//! endpoint, says who is speaking, and sends calls, each answered once.
//! Nothing outside the node writes the World's state; clients ask.
//!
//! Above it, what every interface shares: [`World`], one method for each
//! thing asked of the World; [`TownView`], who is where in the shape to show
//! it; [`Conversation`], talking to a resident and following its turns.
//! A [`device::Device`] supplies the same World API on a computer or phone
//! that runs no node, keeping its identity and network connection in the
//! application's own directory.

pub mod account_records;
mod address;
pub mod again;
mod artifact_sink;
pub mod attachments;
pub mod cache;
mod collab;
mod collaboration;
pub mod composer;
pub mod conversation;
mod data;
pub mod device;
pub mod features;
pub mod file_read;
mod file_search;
mod frozen_lines;
mod frozen_output;
pub mod goals;
pub mod host;
mod layers;
mod line_rows;
mod lines;
mod lines_read;
mod local_watch;
mod management;
pub mod mcp_relay;
pub mod memory;
mod originals;
pub mod pages;
pub mod plans;
mod profile;
mod sentinels;
pub mod renewal;
pub mod shown;
mod thumbnail;
pub mod town;
pub mod world;
pub mod world_now;

pub use address::hub_addresses;
pub use again::{Again, Interruption};
pub use conversation::{Conversation, Following, Moment, Next, Update};
pub use file_read::Reading;
pub use town::TownView;
pub use world::{Error, Said, TownRecord, World};

use fastctx_platform::ipc::{self, Connection, Endpoint};
use fastctx_types::ipc::{ClientFrame, Failure, Hello, NOTICES, NodeFrame, PROTOCOL, Peer, SessionHint, Welcome};
use fastctx_types::layout::WorldDir;
use fastctx_types::session::Watched;
use fastctx_types::{Call, MachineName, Reply};

/// Why there is no connection to the node.
#[derive(Debug, thiserror::Error)]
pub enum ConnectError {
    #[error("the node is not running ({endpoint})")]
    NoNode { endpoint: String },
    #[error("the node refused: {reason}")]
    Refused { reason: String, mismatch: Option<fastctx_types::renewal::Mismatch> },
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

/// How long a World its service keeps may stand with no node before that is
/// taken as the service not starting one: launchd and systemd start an
/// exited node again within a second or two.
const SERVICE_RESTART: std::time::Duration = std::time::Duration::from_secs(10);

/// Whether a node holds the World now, opening its endpoint or on its way
/// out. Asking takes the lock for a moment when it is free; a node waiting
/// for it takes it right after.
fn held(world: &WorldDir) -> std::io::Result<bool> {
    Ok(fastctx_platform::FileLock::try_acquire(&world.node_lock())?.is_none())
}

/// Whether a node waits to take the World, by what it said of itself, while
/// its process lives (`WorldDir::node_successor`).
fn successor_waiting(world: &WorldDir) -> bool {
    std::fs::read(world.node_successor())
        .ok()
        .and_then(|bytes| serde_json::from_slice::<fastctx_types::ProcessIdentity>(&bytes).ok())
        .is_some_and(|successor| fastctx_platform::is_alive(&successor).unwrap_or(false))
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
    pub profiles: bool,
    pub account_records: bool,
    pub conversation_plans: bool,
    pub goal_runtime: bool,
    pub watch_many: bool,
    pub scratch_locations: bool,
    pub sentinels: bool,
    pub goal_commands: bool,
    /// The connected node's facts, including when another starter won.
    pub node_lifetime: Option<Box<fastctx_types::node_lifetime::NodeLifetime>>,
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
        let hello = Hello { protocol: PROTOCOL, version: env!("CARGO_PKG_VERSION").to_string(), peer, notices: NOTICES };
        connection.send(&to_json(&hello)).await.map_err(io_error)?;
        let welcome = connection
            .recv()
            .await
            .map_err(io_error)?
            .ok_or_else(|| io_error(std::io::ErrorKind::UnexpectedEof.into()))?;
        match serde_json::from_slice::<Welcome>(&welcome).map_err(|error| io_error(std::io::Error::other(error)))? {
            Welcome::Accepted {
                machine,
                version,
                node_lifetime,
                profiles,
                account_records,
                conversation_plans,
                goal_runtime,
                watch_many,
                scratch_locations,
                sentinels,
                goal_commands,
            } => Ok(Self {
                connection,
                next: 0,
                machine,
                node_version: version,
                node_lifetime,
                profiles,
                account_records,
                conversation_plans,
                goal_runtime,
                watch_many,
                scratch_locations,
                sentinels,
                goal_commands,
            }),
            Welcome::Refused { reason, mismatch } => {
                let mismatch = mismatch.or_else(|| renewal::legacy_mismatch(&reason));
                Err(ConnectError::Refused { reason, mismatch })
            }
        }
    }

    /// Connects to the node, first starting it if it is not running. The
    /// node uses the user's environment. On Windows, a caller in a job uses
    /// the user's desktop shell as the parent when one exists; without a
    /// shell, `node_lifetime` reports any retained job containment.
    /// A node a system service keeps is started by nobody else: a second
    /// one would hold the World while the service's waits for it.
    pub async fn connect_or_start(world: &WorldDir, peer: Peer) -> Result<Self, ConnectError> {
        Self::connect_or_start_with(world, peer, None).await
    }

    /// As [`Client::connect_or_start`], with an explicit `program` for an
    /// interface that is not fastctx. Without one, the installed copy is
    /// preferred, then this process when the World has no installed copy.
    pub async fn connect_or_start_with(
        world: &WorldDir,
        peer: Peer,
        program: Option<&std::path::Path>,
    ) -> Result<Self, ConnectError> {
        if world.path().parent().is_some_and(|root| root.join("uninstalled.json").exists()) {
            return Err(ConnectError::Refused {
                reason: "this World was uninstalled; reinstall it before starting a node".into(),
                mismatch: None,
            });
        }
        match Self::connect(world, peer.clone()).await {
            // The service's node is on its way while a node holds the World,
            // opening its endpoint once it has looked at what the last one
            // left, or while a successor waits to take it from one handing it
            // over: their ownership and their lives, not a guessed delay,
            // keep the client waiting. Nobody holds it for a moment after a
            // node exits, before the service manager has started the next,
            // as when the node that served before the service was installed
            // hands the World to the service's: that moment is waited out.
            Err(ConnectError::NoNode { endpoint }) if world.node_service().exists() => {
                let mut vacant_since = None;
                loop {
                    if held(world).map_err(|error| ConnectError::Io { endpoint: endpoint.clone(), error })?
                        || successor_waiting(world)
                    {
                        vacant_since = None;
                    } else if vacant_since.get_or_insert_with(std::time::Instant::now).elapsed() >= SERVICE_RESTART {
                        return Err(ConnectError::KeptByService { endpoint });
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                    match Self::connect(world, peer.clone()).await {
                        Err(ConnectError::NoNode { .. }) => {}
                        connected => return connected,
                    }
                }
            }
            Err(ConnectError::NoNode { endpoint }) => {
                let started = start_node(world, program, &peer)
                    .map_err(|error| ConnectError::Io { endpoint: endpoint.clone(), error })?;
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
                        match Self::connect(world, peer.clone()).await {
                            Err(ConnectError::NoNode { .. }) => {
                                match fastctx_platform::FileLock::try_acquire(&world.node_lock()) {
                                    // Another starter owns the World and is still
                                    // opening its endpoint. Its ownership, rather
                                    // than a guessed startup delay, keeps us waiting.
                                    Ok(None) => {}
                                    Ok(Some(_)) => {
                                        return Err(ConnectError::NodeDidNotStart {
                                            log: world.node_log().display().to_string(),
                                        });
                                    }
                                    Err(error) => return Err(ConnectError::Io { endpoint: endpoint.clone(), error }),
                                }
                            }
                            connected => return connected,
                        }
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
        if !self.conversation_plans
            && matches!(call, Call::Plan { .. } | Call::PlanRead { .. } | Call::PlanWrite { .. })
        {
            return Err(CallError::Failed(Failure::not_run(fastctx_types::plan::words::OLDER)));
        }
        if !self.account_records && matches!(call, Call::AccountRecords { .. } | Call::WatchAccountRecords { .. }) {
            return Err(CallError::Failed(Failure::not_run(fastctx_types::account_records::words::NODE_OLD)));
        }
        if !self.goal_runtime && matches!(call, Call::Goal { .. } | Call::GoalCommand { .. }) {
            return Err(CallError::Failed(Failure::not_run(fastctx_types::goals::words::OLDER_NODE)));
        }
        if !self.sentinels && matches!(call, Call::SentinelSettings { .. } | Call::SentinelKey { .. } | Call::Sentinel { .. }) {
            return Err(CallError::Failed(Failure::not_run(format!("upgrade {} to use sentinels", self.machine))));
        }
        if !self.goal_commands
            && matches!(
                call,
                Call::GoalCommand { request: fastctx_types::goals::AgentRequest::ControlColleague { .. }, .. }
            )
        {
            return Err(CallError::Failed(Failure::not_run(fastctx_types::goals::words::OLDER_COMMAND_NODE)));
        }
        if !self.profiles
            && matches!(
                call,
                Call::ProfileRead { .. } | Call::ProfileWrite { .. } | Call::Mcp { .. } | Call::McpRelay { .. }
            )
        {
            return Err(CallError::Failed(Failure::not_run(format!(
                "upgrade {} to use user-level configuration",
                self.machine
            ))));
        }
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
            NodeFrame::AccountRecords { .. }
            | NodeFrame::McpOpened { .. }
            | NodeFrame::McpLine { .. }
            | NodeFrame::Watched { .. }
            | NodeFrame::On { .. }
            | NodeFrame::ToolsChanged { .. }
            | NodeFrame::Features { .. }
            | NodeFrame::World { .. } => Err(broken("the node answered a call as a stream".to_string())),
            NodeFrame::FileRead { .. } => Err(broken("use read_file to consume a file byte stream".to_string())),
            NodeFrame::LinesRead { .. } => Err(broken("use World.lines to consume logical rows".to_string())),
        }
    }

    /// Sends an `On` call; the connection carries the job's output from then
    /// on, and closing it stops the job.
    pub async fn on(mut self, call: Call) -> Result<OnStream, CallError> {
        self.next += 1;
        let id = self.next;
        self.connection.send(&to_json(&ClientFrame::Call { id, call })).await.map_err(CallError::Broken)?;
        Ok(OnStream { client: self, id })
    }

    /// Acknowledge a wait result only after its command has rendered and flushed it.
    pub async fn take_wait(&mut self) -> Result<(), CallError> {
        self.connection.send(&to_json(&fastctx_types::on::TAKEN)).await.map_err(CallError::Broken)?;
        let frame = self
            .connection
            .recv()
            .await
            .map_err(CallError::Broken)?
            .ok_or_else(|| CallError::Broken(std::io::ErrorKind::UnexpectedEof.into()))?;
        match serde_json::from_slice::<NodeFrame>(&frame)
            .map_err(|error| CallError::Broken(std::io::Error::other(error)))?
        {
            NodeFrame::Reply { id, reply } if id == self.next => match reply.map_err(CallError::Failed)? {
                Reply::Done => Ok(()),
                _ => Err(CallError::Broken(std::io::Error::other("the node did not confirm the wait receipt"))),
            },
            _ => Err(CallError::Broken(std::io::Error::other("the node did not answer the wait receipt"))),
        }
    }

    /// Sends a `Watch` call; the connection carries the watch from then on.
    pub async fn watch(mut self, call: Call) -> Result<Watching, CallError> {
        if !self.goal_runtime && matches!(call, Call::Watch { goal: Some(_), .. }) {
            return Err(CallError::Failed(Failure::not_run(fastctx_types::goals::words::OLDER_NODE)));
        }
        self.next += 1;
        let id = self.next;
        self.connection.send(&to_json(&ClientFrame::Call { id, call })).await.map_err(CallError::Broken)?;
        Ok(Watching { transport: WatchingTransport::Local { client: self, id } })
    }

    /// Sends a `WatchFeatures` call; the connection carries the switches
    /// from then on.
    pub(crate) async fn watch_features(mut self) -> Result<(Client, u64), CallError> {
        self.next += 1;
        let id = self.next;
        self.connection
            .send(&to_json(&ClientFrame::Call { id, call: Call::WatchFeatures }))
            .await
            .map_err(CallError::Broken)?;
        Ok((self, id))
    }

    /// Sends a `WatchWorld` call; the connection carries the World as an
    /// interface shows it from then on.
    pub(crate) async fn watch_world(mut self) -> Result<(Client, u64), CallError> {
        self.next += 1;
        let id = self.next;
        self.connection
            .send(&to_json(&ClientFrame::Call { id, call: Call::WatchWorld }))
            .await
            .map_err(CallError::Broken)?;
        Ok((self, id))
    }

    pub(crate) async fn watch_memory(mut self, town: fastctx_types::TownName) -> Result<(Client, u64), CallError> {
        self.next += 1;
        let id = self.next;
        self.connection
            .send(&to_json(&ClientFrame::Call { id, call: Call::WatchMemory { town } }))
            .await
            .map_err(CallError::Broken)?;
        Ok((self, id))
    }

    /// The next frame on a connection given over to one stream.
    pub(crate) async fn frame(&mut self) -> Result<NodeFrame, CallError> {
        let frame = self
            .connection
            .recv()
            .await
            .map_err(CallError::Broken)?
            .ok_or_else(|| CallError::Broken(std::io::ErrorKind::UnexpectedEof.into()))?;
        serde_json::from_slice::<NodeFrame>(&frame).map_err(|error| CallError::Broken(std::io::Error::other(error)))
    }

    /// Sends a `WatchTools` call; the connection says from then on each
    /// time the session's tools may have changed.
    pub async fn watch_tools(mut self, call: Call) -> Result<ToolsWatch, CallError> {
        self.next += 1;
        let id = self.next;
        self.connection.send(&to_json(&ClientFrame::Call { id, call })).await.map_err(CallError::Broken)?;
        Ok(ToolsWatch { client: self, id })
    }
}

/// A session's tools watch; dropping it ends the watch.
pub struct ToolsWatch {
    client: Client,
    id: u64,
}

impl ToolsWatch {
    /// Returns when the tools may have changed; fails once the watch ended.
    pub async fn next(&mut self) -> Result<(), CallError> {
        let frame = self
            .client
            .connection
            .recv()
            .await
            .map_err(CallError::Broken)?
            .ok_or_else(|| CallError::Broken(std::io::ErrorKind::UnexpectedEof.into()))?;
        let broken = |what: String| CallError::Broken(std::io::Error::other(what));
        match serde_json::from_slice::<NodeFrame>(&frame).map_err(|error| broken(error.to_string()))? {
            NodeFrame::ToolsChanged { id } if id == self.id => Ok(()),
            NodeFrame::Reply { id, reply } if id == self.id => match reply {
                Ok(_) => Err(broken("the tools watch ended".to_string())),
                Err(failure) => Err(CallError::Failed(failure)),
            },
            _ => Err(broken("the node answered another call on a tools watch".to_string())),
        }
    }
}

/// An `on` job's output as the node hands it over; dropping it stops the
/// job.
pub struct OnStream {
    client: Client,
    id: u64,
}

impl OnStream {
    /// Deliberately stops this hop when a downstream reader leaves.
    pub async fn stop(&mut self) -> Result<(), CallError> {
        self.client.connection.send(&to_json(&fastctx_types::on::STOP)).await.map_err(CallError::Broken)
    }

    /// Confirms that the CLI read the completion and waits for the node to
    /// finish recording its receipt. This is not a receipt from the model.
    pub async fn taken(&mut self) -> Result<(), CallError> {
        self.client.connection.send(&to_json(&fastctx_types::on::TAKEN)).await.map_err(CallError::Broken)?;
        match self.next().await? {
            None => Ok(()),
            Some(_) => Err(CallError::Broken(std::io::Error::other("the node sent output after the on completion"))),
        }
    }

    /// The next piece; `None` once the call ended as it should.
    pub async fn next(&mut self) -> Result<Option<fastctx_types::on::OnPiece>, CallError> {
        let frame = self
            .client
            .connection
            .recv()
            .await
            .map_err(CallError::Broken)?
            .ok_or_else(|| CallError::Broken(std::io::ErrorKind::UnexpectedEof.into()))?;
        let broken = |what: String| CallError::Broken(std::io::Error::other(what));
        match serde_json::from_slice::<NodeFrame>(&frame).map_err(|error| broken(error.to_string()))? {
            NodeFrame::On { id, piece } if id == self.id => Ok(Some(piece)),
            NodeFrame::Reply { id, reply } if id == self.id => reply.map(|_| None).map_err(CallError::Failed),
            _ => Err(broken("the node answered another call on an on".to_string())),
        }
    }
}

/// A watch the node hands over as it goes; dropping it ends the watch.
pub struct Watching {
    transport: WatchingTransport,
}

enum WatchingTransport {
    Local { client: Client, id: u64 },
    LocalMany(local_watch::Watching),
    Device(device::watch::Watching),
}

impl Watching {
    pub(crate) fn device(watching: device::watch::Watching) -> Self {
        Self { transport: WatchingTransport::Device(watching) }
    }

    /// The next piece; `None` once the watch ended as it should.
    pub async fn next(&mut self) -> Result<Option<Watched>, CallError> {
        let (client, expected) = match &mut self.transport {
            WatchingTransport::Local { client, id } => (client, id),
            WatchingTransport::Device(watching) => return watching.next().await,
            WatchingTransport::LocalMany(watching) => return watching.next().await,
        };
        let frame = client
            .connection
            .recv()
            .await
            .map_err(CallError::Broken)?
            .ok_or_else(|| CallError::Broken(std::io::ErrorKind::UnexpectedEof.into()))?;
        let broken = |what: String| CallError::Broken(std::io::Error::other(what));
        match serde_json::from_slice::<NodeFrame>(&frame).map_err(|error| broken(error.to_string()))? {
            NodeFrame::Watched { id, watched } if id == *expected => Ok(Some(watched)),
            NodeFrame::Reply { id, reply } if id == *expected => reply.map(|_| None).map_err(CallError::Failed),
            _ => Err(broken("the node answered another call on a watch".to_string())),
        }
    }
}

/// Who a command speaks as, from its environment: a resident, when the
/// World's `run` started it; an agent's session, when that agent's own
/// shell did, as its host says; otherwise whoever is at the terminal.
pub fn command_peer() -> std::io::Result<Peer> {
    let invalid = |name: &str| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("invalid {name}: the command has no recognized identity"),
        )
    };
    let set = |name: &str| -> std::io::Result<Option<String>> {
        match std::env::var(name) {
            Ok(value) if !value.is_empty() => Ok(Some(value)),
            Err(std::env::VarError::NotPresent) => Ok(None),
            _ => Err(invalid(name)),
        }
    };
    if let Some(page) = set("FASTCTX_PAGE")? {
        let page = serde_json::from_str(&page).map_err(|_| invalid("FASTCTX_PAGE"))?;
        // The page's script runs as a job; the node checks it is that job.
        let job = set("FASTCTX_JOB")?.map(|job| job.parse().map_err(|_| invalid("FASTCTX_JOB"))).transpose()?;
        return Ok(Peer::Page { page, job });
    }
    let resident =
        set("FASTCTX_RESIDENT")?.map(|value| value.parse().map_err(|_| invalid("FASTCTX_RESIDENT"))).transpose()?;
    let thread = set("CODEX_THREAD_ID")?;
    let inbox = set("CLAUDE_CODE_MESSAGING_SOCKET")?;
    let session = match (thread, inbox) {
        (Some(thread), _) => Some(SessionHint::Codex { thread }),
        (None, Some(inbox)) => Some(SessionHint::Claude { inbox }),
        (None, None) => None,
    };
    Ok(Peer::Command { resident, session })
}

/// Starts a node on its own, from `program` or else this binary.
fn start_node(
    world: &WorldDir,
    program: Option<&std::path::Path>,
    peer: &Peer,
) -> std::io::Result<fastctx_platform::DetachedProcess> {
    let root = world
        .path()
        .parent()
        .filter(|_| world.path().file_name() == Some(std::ffi::OsStr::new("world")))
        .ok_or_else(|| std::io::Error::other("starting a node requires a World under <fastctx root>/world"))?;
    // A long-lived tool adapter must not revive its pinned old node after
    // an update. An uninstalled World still starts from its caller.
    let installed = root.join("bin").join(if cfg!(windows) { "fastctx.exe" } else { "fastctx" });
    let executable = match program {
        Some(program) => program.to_path_buf(),
        None if installed.try_exists()? => installed,
        None => std::env::current_exe()?,
    };
    let home = fastctx_platform::user_home()?;
    let mut environment = fastctx_platform::world_process_environment()?;
    // The caller explicitly named a World. A Claude daemon's inherited
    // FASTCTX_HOME may describe an entirely different one.
    environment.retain(|(name, _)| !name.eq_ignore_ascii_case("FASTCTX_HOME"));
    environment.push(("FASTCTX_HOME".into(), root.as_os_str().to_os_string()));
    let starter = fastctx_types::node_lifetime::NodeStarter {
        process: fastctx_platform::current_identity()?,
        session: match peer {
            Peer::Session { host } => Some(host.clone()),
            _ => None,
        },
    };
    environment.retain(|(name, _)| !name.eq_ignore_ascii_case("FASTCTX_NODE_STARTER"));
    environment
        .push(("FASTCTX_NODE_STARTER".into(), serde_json::to_string(&starter).map_err(std::io::Error::other)?.into()));
    let args = [std::ffi::OsStr::new("node"), std::ffi::OsStr::new("--dir"), root.as_os_str()];
    #[cfg(windows)]
    if fastctx_platform::in_job()?
        && let Some(started) = fastctx_platform::shell_process::spawn(&executable, &args, &home, &environment)?
    {
        return Ok(started);
    }
    fastctx_platform::spawn_detached(&executable, &args, &home, &environment)
}

fn to_json(value: &impl serde::Serialize) -> Vec<u8> {
    serde_json::to_vec(value).expect("the World's types always serialize")
}
