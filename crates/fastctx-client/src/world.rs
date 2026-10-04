//! The World as this machine's node shows it: one method for each thing an
//! interface asks of it. Each call opens its own connection, so calls from
//! several tasks run side by side.

use fastctx_types::copy::{CopyReport, Place};
use fastctx_types::ipc::Peer;
use fastctx_types::layout::WorldDir;
use fastctx_types::project::{JournalSummary, JournalView, MapView, Record, RecordFields, Stamp, TownSummary};
use fastctx_types::residents::Landed;
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
    #[error(transparent)]
    Connect(#[from] ConnectError),
    #[error(transparent)]
    Call(#[from] CallError),
    /// The node answered with something other than what the call asks for:
    /// a node of another version, or a fault in it.
    #[error("the node answered something else: {0}")]
    Unexpected(String),
}

/// A World, reached through its node on this machine, speaking as `peer`.
#[derive(Clone)]
pub struct World {
    dir: WorldDir,
    peer: Peer,
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
        Self { dir, peer }
    }

    /// The World kept for this user on this machine, speaking as whoever
    /// runs the program (`crate::command_peer`).
    pub fn here() -> std::io::Result<Self> {
        Ok(Self::new(WorldDir::under(&fastctx_platform::fastctx_root()?), crate::command_peer()))
    }

    pub fn dir(&self) -> &WorldDir {
        &self.dir
    }

    async fn connect(&self) -> Result<Client, Error> {
        Ok(Client::connect_or_start(&self.dir, self.peer.clone()).await?)
    }

    /// Sends one call and returns its answer and the machine that answered.
    pub async fn call(&self, call: Call) -> Result<(Reply, MachineName), Error> {
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

    /// The towns this machine is part of.
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
        match self.ask(Call::Talk { to, machine, text }).await? {
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
        match self.ask(Call::Open { agent, town, node, text }).await? {
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

    /// A code for one more machine to join with, until `expires`.
    pub async fn invite(&self, name: Option<MachineName>, expires: Timestamp) -> Result<String, Error> {
        match self.ask(Call::Invite { name, expires }).await? {
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
