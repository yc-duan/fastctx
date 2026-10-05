//! One World API for local-node interfaces and devices without a node.
//! Local calls use independent IPC connections; device calls share one
//! persistent link and are addressed to their machine or a relay.

use fastctx_types::copy::{CopyReport, Place};
use fastctx_types::ipc::Peer;
use fastctx_types::layout::WorldDir;
use fastctx_types::project::{JournalSummary, JournalView, MapView, Record, RecordFields, Stamp, TownSummary};
use fastctx_types::residents::Landed;
use fastctx_types::session::control::{Control, Controlled, Offers};
use fastctx_types::town::{LookedUp, WorldWho};
use fastctx_types::world::NodesReply;
use fastctx_types::{
    Address, AgentName, Attach, Call, EntryId, JournalCall, JournalName, MachineName, ProjectCall, Reply, ResidentId,
    TownName,
};
use jiff::Timestamp;

use crate::{CallError, Client, ConnectError, Watching};

/// Why something asked of the World was not done.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("keeping the device identity: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    FileRead(#[from] fastctx_types::file_read::FileReadError),
    #[error(transparent)]
    Connect(#[from] ConnectError),
    #[error(transparent)]
    Call(#[from] CallError),
    /// The node answered with something other than what the call asks for:
    /// a node of another version, or a fault in it.
    #[error("the node answered something else: {0}")]
    Unexpected(String),
}

/// A World reached through a local node or an application's device identity.
#[derive(Clone)]
pub struct World {
    dir: WorldDir,
    peer: Peer,
    /// The program a node is started from when none runs; this process's
    /// own executable when unset.
    node_program: Option<std::path::PathBuf>,
    device: Option<std::sync::Arc<crate::device::Device>>,
}

/// What became of words said to a resident.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Said {
    /// Where the resident lives.
    pub to: Address,
    pub landed: Landed,
}

/// A town's record as its latest version says, and who opened it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TownRecord {
    pub town: TownName,
    pub record: Record,
    pub opened: Stamp,
}

impl World {
    pub fn new(dir: WorldDir, peer: Peer) -> Self {
        Self { dir, peer, node_program: None, device: None }
    }

    /// The same interface over a device's persistent network connection.
    /// No local node is discovered or started by this World.
    pub fn from_device(device: std::sync::Arc<crate::device::Device>) -> Self {
        Self {
            dir: WorldDir::at(device.directory()),
            peer: Peer::Command { resident: None, session: None },
            node_program: None,
            device: Some(device),
        }
    }

    pub fn device(&self) -> Option<&std::sync::Arc<crate::device::Device>> {
        self.device.as_ref()
    }

    /// The same World, whose node, when none runs, is started from
    /// `program`: for an interface that is not itself fastctx.
    pub fn starting_node_from(mut self, program: std::path::PathBuf) -> Self {
        self.node_program = Some(program);
        self
    }

    /// The World kept for this user on this machine, speaking as whoever
    /// runs the program (`crate::command_peer`).
    pub fn here() -> std::io::Result<Self> {
        Ok(Self::new(WorldDir::under(&fastctx_platform::fastctx_root()?), crate::command_peer()))
    }

    /// The local node's World directory, or the application's private
    /// device directory. A device directory holds no local node or towns.
    pub fn dir(&self) -> &WorldDir {
        &self.dir
    }

    pub(crate) async fn connect(&self) -> Result<Client, Error> {
        Ok(Client::connect_or_start_with(&self.dir, self.peer.clone(), self.node_program.as_deref()).await?)
    }

    /// Sends one call and returns its answer and the machine that answered.
    pub async fn call(&self, call: Call) -> Result<(Reply, MachineName), Error> {
        if let Some(device) = &self.device {
            return device.call(call).await;
        }
        let mut client = self.connect().await?;
        let reply = client.call(call).await?;
        Ok((reply, client.machine.clone()))
    }

    async fn ask(&self, call: Call) -> Result<Reply, Error> {
        Ok(self.call(call).await?.0)
    }

    /// Who is in `town`, or in the caller's, or across the World for a
    /// caller in none; with `all`, also those who are not here.
    pub async fn who(&self, town: Option<TownName>, all: bool) -> Result<WorldWho, Error> {
        match self.ask(Call::Who { all, town }).await? {
            Reply::Who(who) => Ok(who),
            other => Err(unexpected(other)),
        }
    }

    /// Facts recorded strictly after `since`, in chronological order, from
    /// `town`'s machines or the whole World. Inspect machine outcomes, read
    /// issues and limitations alongside the timeline. Undated facts are
    /// returned separately and do not establish what happened since departure.
    pub async fn since(
        &self,
        since: Timestamp,
        town: Option<TownName>,
    ) -> Result<fastctx_types::since::WorldSince, Error> {
        match self.ask(Call::Since { since, town }).await? {
            Reply::Since(since) => Ok(since),
            other => Err(unexpected(other)),
        }
    }

    /// Residents or jobs in detail, by id.
    pub async fn look(&self, ids: Vec<String>) -> Result<Vec<LookedUp>, Error> {
        match self.ask(Call::Look { ids }).await? {
            Reply::Looked { looked } => Ok(looked),
            other => Err(unexpected(other)),
        }
    }

    /// The machines of the World.
    pub async fn nodes(&self) -> Result<NodesReply, Error> {
        match self.ask(Call::Nodes).await? {
            Reply::Nodes(nodes) => Ok(nodes),
            other => Err(unexpected(other)),
        }
    }

    /// Queries one machine, or fans out to every online machine. Offline
    /// and unanswered machines remain explicit in the result.
    pub async fn agents(&self, machine: Option<MachineName>) -> Result<fastctx_types::agents::WorldAgents, Error> {
        match self.ask(Call::Agents { machine }).await? {
            Reply::Agents(agents) => Ok(agents),
            other => Err(unexpected(other)),
        }
    }

    /// The World's projects, including those that do not span this machine.
    pub async fn towns(&self) -> Result<Vec<TownSummary>, Error> {
        match self.ask(Call::Project(ProjectCall::List)).await? {
            Reply::Towns { towns } => Ok(towns),
            other => Err(unexpected(other)),
        }
    }

    pub async fn town(&self, town: Option<TownName>) -> Result<TownRecord, Error> {
        record(self.ask(Call::Project(ProjectCall::Show { town })).await?)
    }

    pub async fn map(&self, town: Option<TownName>) -> Result<MapView, Error> {
        match self.ask(Call::Project(ProjectCall::Map { town })).await? {
            Reply::Map { map } => Ok(*map),
            other => Err(unexpected(other)),
        }
    }

    /// Opens a town with the first version of its record.
    pub async fn open_town(&self, town: TownName, fields: RecordFields) -> Result<TownRecord, Error> {
        record(self.ask(Call::Project(ProjectCall::New { town, fields })).await?)
    }

    /// Writes the next version of a town's record.
    pub async fn edit_town(&self, town: Option<TownName>, fields: RecordFields) -> Result<TownRecord, Error> {
        record(self.ask(Call::Project(ProjectCall::Edit { town, fields })).await?)
    }

    /// The caller, a resident, lives in `town` from now on; returns its map.
    pub async fn enter(&self, town: TownName) -> Result<MapView, Error> {
        match self.ask(Call::Project(ProjectCall::Enter { town })).await? {
            Reply::Entered { map, .. } => Ok(*map),
            other => Err(unexpected(other)),
        }
    }

    pub async fn journals(&self, town: Option<TownName>) -> Result<(TownName, Vec<JournalSummary>), Error> {
        match self.ask(Call::Journal(JournalCall::List { town })).await? {
            Reply::Journals { town, journals } => Ok((town, journals)),
            other => Err(unexpected(other)),
        }
    }

    pub async fn journal(
        &self,
        town: Option<TownName>,
        journal: JournalName,
        before: Option<EntryId>,
    ) -> Result<JournalView, Error> {
        match self.ask(Call::Journal(JournalCall::Read { town, journal, before })).await? {
            Reply::Journal { view } => Ok(*view),
            other => Err(unexpected(other)),
        }
    }

    /// The user's words to a resident on any machine.
    pub async fn talk(&self, to: ResidentId, machine: Option<MachineName>, text: String) -> Result<Said, Error> {
        match self.ask(Call::Talk { to, machine, text, attachments: Vec::new() }).await? {
            Reply::Spoken { to, landed } => Ok(Said { to, landed }),
            other => Err(unexpected(other)),
        }
    }

    /// Opens a session for the user, with the user's words as its first.
    pub async fn open(
        &self,
        agent: AgentName,
        town: Option<TownName>,
        node: Option<MachineName>,
        text: String,
    ) -> Result<Said, Error> {
        self.open_with_options(agent, town, node, text, fastctx_types::agents::SessionOptions::default()).await
    }

    /// Opens with the user's model and reasoning effort selections. Values
    /// are the official program's identifiers; omitted values use its defaults.
    pub async fn open_with_options(
        &self,
        agent: AgentName,
        town: Option<TownName>,
        node: Option<MachineName>,
        text: String,
        options: fastctx_types::agents::SessionOptions,
    ) -> Result<Said, Error> {
        match self
            .ask(Call::Open {
                agent,
                town,
                node,
                text,
                model: options.model,
                reasoning_effort: options.reasoning_effort,
                attachments: Vec::new(),
            })
            .await?
        {
            Reply::Opened { resident, landed } => Ok(Said { to: resident, landed }),
            other => Err(unexpected(other)),
        }
    }

    /// Stops the turn a resident on any machine is running; returns where it
    /// lives and whether a turn was running.
    pub async fn stop(&self, resident: ResidentId, machine: Option<MachineName>) -> Result<(Address, bool), Error> {
        match self.ask(Call::Stop { resident, machine }).await? {
            Reply::Stopped { resident, stopped } => Ok((resident, stopped)),
            other => Err(unexpected(other)),
        }
    }

    /// Does something to a resident's conversation for the user, besides
    /// speaking into it.
    pub async fn control(
        &self,
        resident: ResidentId,
        machine: Option<MachineName>,
        control: Control,
    ) -> Result<(Address, Controlled), Error> {
        match self.ask(Call::Control { resident, machine, control }).await? {
            Reply::Controlled { resident, controlled } => Ok((resident, controlled)),
            other => Err(unexpected(other)),
        }
    }

    /// What a resident's conversation offers now.
    pub async fn offers(&self, resident: ResidentId, machine: Option<MachineName>) -> Result<(Address, Offers), Error> {
        match self.ask(Call::Offers { resident, machine }).await? {
            Reply::Offers { resident, offers } => Ok((resident, *offers)),
            other => Err(unexpected(other)),
        }
    }

    /// The command that opens a resident of this machine in its host's own
    /// interface, for the user to run in a terminal.
    pub async fn attach(&self, resident: ResidentId) -> Result<fastctx_types::calls::HostCommand, Error> {
        match self.ask(Call::Attach { resident }).await? {
            Reply::Attachable(command) => Ok(command),
            other => Err(unexpected(other)),
        }
    }

    /// Watches a resident on any machine: its history, then each event as
    /// it happens.
    pub async fn watch(&self, resident: ResidentId, machine: Option<MachineName>) -> Result<Watching, Error> {
        if let Some(device) = &self.device {
            return Ok(Watching::device(device.watch(resident, machine).await?));
        }
        Ok(self.connect().await?.watch(Call::Watch { resident, machine }).await?)
    }

    pub async fn copy(&self, from: Place, to: Place) -> Result<CopyReport, Error> {
        match self.ask(Call::Copy { from, to }).await? {
            Reply::Copied(report) => Ok(report),
            other => Err(unexpected(other)),
        }
    }

    /// Joins a World with a code; returns the World's id and this machine's
    /// name in it.
    pub async fn join(&self, code: String, name: Option<MachineName>) -> Result<(String, MachineName), Error> {
        match self.ask(Call::Join { code, name }).await? {
            Reply::Joined { world, name } => Ok((world, name)),
            other => Err(unexpected(other)),
        }
    }

    /// A code for one more machine, or a device, to join with, until
    /// `expires`.
    pub async fn invite(&self, name: Option<MachineName>, expires: Timestamp, device: bool) -> Result<String, Error> {
        match self.ask(Call::Invite { name, expires, device }).await? {
            Reply::Code { code } => Ok(code),
            other => Err(unexpected(other)),
        }
    }

    pub async fn revoke(&self, name: MachineName, reason: String) -> Result<(), Error> {
        done(self.ask(Call::Revoke { name, reason }).await?)
    }

    pub async fn leave(&self) -> Result<(), Error> {
        done(self.ask(Call::Leave).await?)
    }

    /// Asks the node to hand the World over once idle and exit: a service
    /// now keeps this World's node.
    pub async fn hand_over(&self) -> Result<(), Error> {
        done(self.ask(Call::HandOver).await?)
    }

    /// Adds a journal entry.
    #[allow(clippy::too_many_arguments)]
    pub async fn add_entry(
        &self,
        town: Option<TownName>,
        journal: JournalName,
        text: String,
        description: Option<String>,
        cites: Vec<EntryId>,
        attach: Vec<Attach>,
        model: Option<String>,
    ) -> Result<Reply, Error> {
        self.ask(Call::Journal(JournalCall::Add { town, journal, text, description, cites, attach, model })).await
    }
}

fn record(reply: Reply) -> Result<TownRecord, Error> {
    match reply {
        Reply::Record { town, record, opened } => Ok(TownRecord { town, record: *record, opened }),
        other => Err(unexpected(other)),
    }
}

fn done(reply: Reply) -> Result<(), Error> {
    match reply {
        Reply::Done => Ok(()),
        other => Err(unexpected(other)),
    }
}

fn unexpected(reply: Reply) -> Error {
    Error::Unexpected(format!("{reply:?}"))
}
