//! One World API for local-node interfaces and devices without a node.
//! Local calls use independent IPC connections, except that negotiated watches
//! share an observation connection. Device calls share one persistent link and
//! are addressed to their machine or a relay.

use fastctx_types::access::Written;
use fastctx_types::accounts::{AccountChange, AccountsReply};
use fastctx_types::copy::{CopyReport, Place};
use fastctx_types::ipc::Peer;
use fastctx_types::layout::WorldDir;
use fastctx_types::project::{JournalSummary, JournalView, MapView, Record, RecordFields, Stamp, TownSummary};
use fastctx_types::residents::Landed;
use fastctx_types::session::control::{Control, Controlled, Offers};
use fastctx_types::town::{LookedUp, WorldWho};
use fastctx_types::world::NodesReply;
use fastctx_types::{
    AccountName, Address, AgentName, Attach, Call, EntryId, HubName, JobId, JournalCall, JournalName, MachineName,
    ProjectCall, Reply, ResidentId, TownName,
};
use jiff::Timestamp;

use crate::{CallError, Client, ConnectError, Watching};

/// Why something asked of the World was not done.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("keeping the device identity: {0}")]
    Io(#[from] std::io::Error),
    /// What this device keeps of the conversations it viewed could not be
    /// read or written (`crate::cache`).
    #[error("this device's copy of the conversation: {0}")]
    Kept(std::io::Error),
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
    /// What was given could not be used, as said.
    #[error("{0}")]
    Other(String),
}

impl Error {
    /// Whether it says the conversation asked about was deleted: asking
    /// again will not find it.
    pub fn is_deleted(&self) -> bool {
        matches!(self, Error::Call(CallError::Failed(failure)) if failure.is_deleted())
    }

    /// Whether the machine refused this account: a watch that was refused
    /// is not opened again on a timer.
    pub fn is_refused(&self) -> bool {
        matches!(self, Error::Call(CallError::Failed(failure)) if failure.is_refused())
    }
}

/// A World reached through a local node or an application's device identity.
#[derive(Clone)]
pub struct World {
    dir: WorldDir,
    peer: Peer,
    /// The program a node is started from when none runs; the installed
    /// copy when unset, or this process when the World is not installed.
    node_program: Option<std::path::PathBuf>,
    device: Option<std::sync::Arc<crate::device::Device>>,
    pub(crate) searches: std::sync::Arc<std::sync::Mutex<crate::file_search::Searches>>,
    pub(crate) line_sources: std::sync::Arc<std::sync::Mutex<crate::lines::Sources>>,
    pub(crate) local_watches: std::sync::Arc<crate::local_watch::Pool>,
    /// What this device keeps of the conversations it views, when it keeps
    /// them (`crate::cache`).
    cache: Option<std::sync::Arc<crate::cache::Cache>>,
    /// Told when a stream that broke should be opened again now
    /// (`crate::again`).
    pub(crate) retry: std::sync::Arc<tokio::sync::Notify>,
}

/// What became of words said to a resident.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Said {
    /// Where the resident lives.
    pub to: Address,
    pub landed: Landed,
    /// The words' id on that machine, when the device named them.
    pub delivery: Option<fastctx_types::NoticeId>,
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
        Self {
            dir,
            peer,
            node_program: None,
            device: None,
            searches: Default::default(),
            line_sources: Default::default(),
            local_watches: Default::default(),
            cache: None,
            retry: Default::default(),
        }
    }

    /// The same interface over a device's persistent network connection.
    /// No local node is discovered or started by this World.
    pub fn from_device(device: std::sync::Arc<crate::device::Device>) -> Self {
        Self {
            dir: WorldDir::at(device.directory()),
            peer: Peer::Command { resident: None, session: None },
            node_program: None,
            device: Some(device),
            searches: Default::default(),
            line_sources: Default::default(),
            local_watches: Default::default(),
            cache: None,
            retry: Default::default(),
        }
    }

    /// The same World, keeping what it views of conversations in `cache`.
    pub fn keeping(mut self, cache: std::sync::Arc<crate::cache::Cache>) -> Self {
        self.cache = Some(cache);
        self
    }

    /// The same World keeping nothing of what it views: for an interface
    /// whose World is being removed, which writes nothing more there.
    pub fn without_cache(mut self) -> Self {
        self.cache = None;
        self
    }

    /// What this device keeps of the conversations it views, if it keeps
    /// them.
    pub fn cache(&self) -> Option<&std::sync::Arc<crate::cache::Cache>> {
        self.cache.as_ref()
    }

    pub fn device(&self) -> Option<&std::sync::Arc<crate::device::Device>> {
        self.device.as_ref()
    }

    /// Whether no answer came to what was asked, so asking again as it was
    /// asked, under the same id, is how it goes on (`world-network.md`,
    /// 断开不是结束): the node or the machine that was to do it was not
    /// reached, or its answer was lost.
    pub fn unanswered(error: &Error) -> bool {
        match error {
            Error::Connect(_) | Error::Call(CallError::Broken(_)) => true,
            Error::Call(CallError::Failed(failure)) => failure.is_unanswered(),
            _ => false,
        }
    }

    /// Everything that broke is tried again now, not after its wait: this
    /// device's network changed, it woke, or an interface came to the
    /// front. A device dials its hubs again too.
    pub fn retry_now(&self) {
        if let Some(device) = &self.device {
            device.reconnect();
        }
        self.retry.notify_waiters();
    }

    /// Resolves once a way to `machine` opens that was not there when this
    /// was asked (`crate::again`): on a device, through its own links; else
    /// as this computer's node says, which is not started for it. Without
    /// a node that answers, it never resolves, and the wait keeps the pace.
    pub(crate) fn reopened(&self, machine: &Written) -> std::pin::Pin<Box<dyn Future<Output = ()> + Send>> {
        if let Some(device) = &self.device {
            return Box::pin(device.reopened(machine));
        }
        let (dir, peer, machine) = (self.dir.clone(), self.peer.clone(), machine.clone());
        Box::pin(async move {
            let asked = async { Client::connect(&dir, peer).await.ok()?.call(Call::Reopened { machine }).await.ok() };
            if !matches!(asked.await, Some(Reply::Done)) {
                std::future::pending::<()>().await;
            }
        })
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
        Ok(Self::new(WorldDir::under(&fastctx_platform::fastctx_root()?), crate::command_peer()?))
    }

    /// The World kept for this user on this machine, as `World::here`,
    /// keeping what it views of conversations where every interface on this
    /// computer keeps it (`crate::cache::Cache::under`).
    pub fn here_keeping() -> std::io::Result<Self> {
        let root = fastctx_platform::fastctx_root()?;
        Ok(Self::new(WorldDir::under(&root), crate::command_peer()?).keeping(crate::cache::Cache::under(&root)?))
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

    pub(crate) async fn ask(&self, call: Call) -> Result<Reply, Error> {
        Ok(self.call(call).await?.0)
    }

    /// Who is in `town`, or in the caller's, or across the World for a
    /// caller in none; with `all`, also those who are not here.
    pub async fn who(&self, town: Option<TownName>, all: bool) -> Result<WorldWho, Error> {
        match self.ask(Call::Who { all, town, since: None }).await? {
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
    pub async fn agents(&self, machine: Option<Written>) -> Result<fastctx_types::agents::WorldAgents, Error> {
        match self.ask(Call::Agents { machine }).await? {
            Reply::Agents(agents) => Ok(agents),
            other => Err(unexpected(other)),
        }
    }

    /// What the nodes of one machine, or of every machine this user may
    /// look at, keep of their agents' logins; no program is started.
    pub async fn agent_logins(&self, machine: Option<Written>) -> Result<fastctx_types::logins::WorldLogins, Error> {
        match self.ask(Call::AgentLogins { machine }).await? {
            Reply::AgentLogins(logins) => Ok(logins),
            other => Err(unexpected(other)),
        }
    }

    /// Scans `machine` for its agents, answered once the usual places are
    /// done while its chosen folders are searched again; with `stop`, stops
    /// those searches.
    pub async fn scan_agents(
        &self,
        machine: Written,
        stop: bool,
    ) -> Result<fastctx_types::logins::MachineLogins, Error> {
        machine_logins(self.ask(Call::ScanAgents { machine, stop }).await?)
    }

    /// Takes up `dir` on `machine` as a candidate login of `agent`.
    pub async fn add_agent_login(
        &self,
        machine: Written,
        agent: fastctx_types::AgentName,
        dir: std::path::PathBuf,
    ) -> Result<fastctx_types::logins::MachineLogins, Error> {
        machine_logins(self.ask(Call::AddAgentLogin { machine, agent, dir }).await?)
    }

    /// Chooses the folder `dir` on `machine` for logins, stops its search,
    /// or no longer searches it.
    pub async fn agent_folder(
        &self,
        machine: Written,
        dir: std::path::PathBuf,
        change: fastctx_types::logins::FolderChange,
    ) -> Result<fastctx_types::logins::MachineLogins, Error> {
        machine_logins(self.ask(Call::AgentFolder { machine, dir, change }).await?)
    }

    /// Turns the automatic naming of Codex conversations in a login on or
    /// off (`world-agent-logins.md`, 在哪个登录里干活).
    pub async fn set_agent_title(
        &self,
        machine: Written,
        agent: fastctx_types::AgentName,
        dir: std::path::PathBuf,
        on: bool,
    ) -> Result<fastctx_types::logins::MachineLogins, Error> {
        machine_logins(self.ask(Call::SetAgentTitle { machine, agent, dir, on }).await?)
    }

    /// What putting the World's tools into a login (`into`), or taking them
    /// out, would change; with `take_back`, rewriting what the user changed
    /// of them too.
    pub async fn preview_agent_change(
        &self,
        machine: Written,
        agent: fastctx_types::AgentName,
        dir: std::path::PathBuf,
        into: bool,
        take_back: bool,
    ) -> Result<fastctx_types::logins::ChangePreview, Error> {
        match self.ask(Call::PreviewAgentChange { machine, agent, dir, into, take_back }).await? {
            Reply::AgentChangePreview(preview) => Ok(*preview),
            other => Err(unexpected(other)),
        }
    }

    /// Puts the World's tools into a login, or takes them out, by its
    /// machine's node.
    pub async fn change_agent_login(
        &self,
        machine: Written,
        agent: fastctx_types::AgentName,
        dir: std::path::PathBuf,
        into: bool,
        raise_output_limit: bool,
        take_back: bool,
    ) -> Result<fastctx_types::logins::LoginChange, Error> {
        match self.ask(Call::ChangeAgentLogin { machine, agent, dir, into, raise_output_limit, take_back }).await? {
            Reply::AgentLoginChanged(change) => Ok(*change),
            other => Err(unexpected(other)),
        }
    }

    /// Reads a login's limits afresh on its machine.
    pub async fn read_agent_limits(
        &self,
        machine: Written,
        agent: fastctx_types::AgentName,
        dir: std::path::PathBuf,
    ) -> Result<fastctx_types::logins::MachineLogins, Error> {
        machine_logins(self.ask(Call::ReadAgentLimits { machine, agent, dir }).await?)
    }

    /// What a login on a machine was used for, as this computer's user may
    /// see it.
    pub async fn agent_calls(
        &self,
        machine: Written,
        agent: fastctx_types::AgentName,
        dir: std::path::PathBuf,
        query: fastctx_types::usage_calls::UsageCallsQuery,
    ) -> Result<fastctx_types::usage_calls::UsageCallsPage, crate::Error> {
        match self.ask(Call::AgentCalls { machine, agent, dir, query }).await? {
            Reply::AgentCalls(page) => Ok(*page),
            other => Err(unexpected(other)),
        }
    }

    pub async fn agent_usage(
        &self,
        machine: Written,
        agent: fastctx_types::AgentName,
        dir: std::path::PathBuf,
    ) -> Result<fastctx_types::usage::UsageReport, Error> {
        match self.ask(Call::AgentUsage { machine, agent, dir }).await? {
            Reply::AgentUsage(report) => Ok(*report),
            other => Err(unexpected(other)),
        }
    }

    /// The user's word on whether Claude Code on `machine`, in its home
    /// `home` or its default one, may run the World's sessions without
    /// approvals. Answers what it now records and whether that changed.
    pub async fn claude_consent(
        &self,
        machine: Written,
        accept: bool,
        home: Option<std::path::PathBuf>,
    ) -> Result<(bool, bool), Error> {
        match self.ask(Call::ClaudeConsent { machine, accept, home }).await? {
            Reply::ClaudeConsent { accepted, changed, .. } => Ok((accepted, changed)),
            other => Err(unexpected(other)),
        }
    }

    /// The World's projects, including those that do not span this machine.
    /// When its hub is disconnected, a node lists the copies it already has;
    /// `nodes` reports that connection state alongside the available projects.
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

    /// Opens a town with the first version of its record, in `account` of
    /// `hub`, or in the one account the caller works for.
    pub async fn open_town(
        &self,
        town: TownName,
        fields: RecordFields,
        account: Option<AccountName>,
        hub: Option<HubName>,
    ) -> Result<TownRecord, Error> {
        record(self.ask(Call::Project(ProjectCall::New { town, fields, account, hub })).await?)
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
    pub async fn talk(&self, to: ResidentId, machine: Option<Written>, text: String) -> Result<Said, Error> {
        match self.ask(Call::Talk { to, machine, text, attachments: Vec::new(), files_on: None, id: None }).await? {
            Reply::Spoken { to, landed, delivery } => Ok(Said { to, landed, delivery }),
            other => Err(unexpected(other)),
        }
    }

    /// Opens a session for the user, with the user's words as its first.
    pub async fn open(
        &self,
        agent: AgentName,
        town: Option<TownName>,
        node: Option<Written>,
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
        node: Option<Written>,
        text: String,
        options: fastctx_types::agents::SessionOptions,
    ) -> Result<Said, Error> {
        self.open_with(agent, town, node, text, options, Default::default()).await
    }

    /// Opens with the user's selections and choices: the account it works
    /// for, the login it uses, where it works.
    pub async fn open_with(
        &self,
        agent: AgentName,
        town: Option<TownName>,
        node: Option<Written>,
        text: String,
        options: fastctx_types::agents::SessionOptions,
        choices: fastctx_types::agents::Choices,
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
                choices,
            })
            .await?
        {
            Reply::Opened { resident, landed } => Ok(Said { to: resident, landed, delivery: None }),
            other => Err(unexpected(other)),
        }
    }

    /// Stops the turn a resident on any machine is running; returns where it
    /// lives and whether a turn was running.
    pub async fn stop(&self, resident: ResidentId, machine: Option<Written>) -> Result<(Address, bool), Error> {
        match self.ask(Call::Stop { resident, machine }).await? {
            Reply::Stopped { resident, stopped } => Ok((resident, stopped)),
            other => Err(unexpected(other)),
        }
    }

    /// Stops a job and everything it started, as the user: its resident is
    /// told the user stopped it. `machine` is where it runs, this one when
    /// absent.
    pub async fn stop_job(&self, job: JobId, machine: Option<Written>) -> Result<(), Error> {
        match self
            .ask(Call::StopJob {
                job,
                hub: machine.as_ref().and_then(|machine| machine.hub.clone()),
                machine: machine.map(|machine| machine.name),
            })
            .await?
        {
            Reply::JobStopped { .. } => Ok(()),
            other => Err(unexpected(other)),
        }
    }

    /// Does something to a resident's conversation for the user, besides
    /// speaking into it.
    pub async fn control(
        &self,
        resident: ResidentId,
        machine: Option<Written>,
        control: Control,
    ) -> Result<(Address, Controlled), Error> {
        match self.ask(Call::Control { resident, machine, control }).await? {
            Reply::Controlled { resident, controlled } => Ok((resident, controlled)),
            other => Err(unexpected(other)),
        }
    }

    /// What a resident's conversation offers now.
    pub async fn offers(&self, resident: ResidentId, machine: Option<Written>) -> Result<(Address, Offers), Error> {
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

    /// The host's own record of one item of a resident's conversation: what
    /// a slimmed history leaves out, for when the user would see it.
    pub async fn item_detail(
        &self,
        resident: ResidentId,
        machine: Written,
        turn: Option<String>,
        item: String,
    ) -> Result<Option<serde_json::Value>, Error> {
        let call = Call::ItemDetail { resident, machine: Some(machine), turn, item };
        match self.ask(call).await? {
            Reply::ItemDetail { detail, .. } => Ok(detail),
            other => Err(unexpected(other)),
        }
    }

    /// What one of the host's own agents in a resident's conversation did,
    /// read from its own record on the resident's machine.
    pub async fn agent_activity(
        &self,
        resident: ResidentId,
        machine: Written,
        agent: String,
    ) -> Result<fastctx_types::session::AgentActivity, Error> {
        let call = Call::AgentActivity { resident, machine: Some(machine), agent };
        match self.ask(call).await? {
            Reply::AgentActivity { activity, .. } => Ok(*activity),
            other => Err(unexpected(other)),
        }
    }

    /// Watches a resident on any machine: its history, then each event as
    /// it happens.
    pub async fn watch(&self, resident: ResidentId, machine: Option<Written>) -> Result<Watching, Error> {
        self.watch_with_composer(resident, machine, None).await
    }

    /// Watches a resident on any machine, saying what this viewer holds of
    /// its history (`fastctx_types::session::held`): its history comes as
    /// the segments it lacks, then each event as it happens.
    pub async fn watch_held(
        &self,
        resident: ResidentId,
        machine: Option<Written>,
        held: fastctx_types::session::held::Held,
    ) -> Result<Watching, Error> {
        self.watch_as(resident, machine, None, Some(held), false).await
    }

    /// Watches a resident for an interface: as [`World::watch_held`], or from
    /// its whole history without `held`, and told whether it works now
    /// (`Watched::Now`) where its machine tells that.
    pub(crate) async fn watch_shown(
        &self,
        resident: ResidentId,
        machine: Option<Written>,
        held: Option<fastctx_types::session::held::Held>,
    ) -> Result<Watching, Error> {
        self.watch_as(resident, machine, None, held, true).await
    }

    pub(crate) async fn watch_with_composer(
        &self,
        resident: ResidentId,
        machine: Option<Written>,
        composer: Option<String>,
    ) -> Result<Watching, Error> {
        self.watch_as(resident, machine, composer, None, false).await
    }

    async fn watch_as(
        &self,
        resident: ResidentId,
        machine: Option<Written>,
        composer: Option<String>,
        held: Option<fastctx_types::session::held::Held>,
        now: bool,
    ) -> Result<Watching, Error> {
        if let Some(device) = &self.device {
            return Ok(Watching::device(device.watch(resident, machine, composer, held, now, None).await?));
        }
        self.local_watches
            .watch(
                self,
                fastctx_types::watching::local::Subscription { resident, machine, composer, held, now, goal: None },
            )
            .await
    }

    pub async fn copy(&self, from: Place, to: Place) -> Result<CopyReport, Error> {
        match self.ask(Call::Copy { from, to }).await? {
            Reply::Copied(report) => Ok(report),
            other => Err(unexpected(other)),
        }
    }

    /// Logs this computer in to `account` on the hub at `hub`; returns the
    /// hub's name, this computer's name there and the account.
    pub async fn login(
        &self,
        hub: Vec<std::net::SocketAddr>,
        account: AccountName,
        password: String,
        register: bool,
        name: Option<MachineName>,
    ) -> Result<(HubName, MachineName, AccountName), Error> {
        logged_in(self.ask(Call::Login { hub, account, password, register, name }).await?)
    }

    /// What `token` lets this computer into, asked of its hub before it is
    /// used; none from a hub that says nothing of a token before.
    pub async fn peek_token(&self, token: String) -> Result<Option<fastctx_types::accounts::TokenPeek>, Error> {
        match self.ask(Call::PeekToken { token }).await? {
            Reply::TokenPeek { peeked } => Ok(peeked),
            other => Err(unexpected(other)),
        }
    }

    /// Logs this computer in with an enrollment token.
    pub async fn login_with_token(
        &self,
        token: String,
        name: Option<MachineName>,
    ) -> Result<(HubName, MachineName, AccountName), Error> {
        logged_in(self.ask(Call::LoginWithToken { token, name }).await?)
    }

    /// Opens an account on a hub that lets anyone, and logs in to it.
    pub async fn open_account(
        &self,
        hub: Vec<std::net::SocketAddr>,
        account: AccountName,
        password: String,
        register: bool,
        name: Option<MachineName>,
    ) -> Result<(HubName, MachineName, AccountName), Error> {
        logged_in(self.ask(Call::OpenAccount { hub, account, password, register, name }).await?)
    }

    /// Makes `change` as `account` on the hub `hub`, or the one hub; a
    /// token, when it made one.
    pub async fn account(
        &self,
        hub: Option<HubName>,
        account: AccountName,
        change: AccountChange,
    ) -> Result<Option<String>, Error> {
        match self.ask(Call::Account { hub, account, change }).await? {
            Reply::Changed { token } => Ok(token),
            other => Err(unexpected(other)),
        }
    }

    /// Adds an agent's home on `machine`, or hides one or shows it again.
    pub async fn homes(&self, machine: Written, change: fastctx_types::agents::HomeChange) -> Result<(), Error> {
        done(self.ask(Call::Homes { machine, change }).await?)
    }

    /// The folders in a folder of `machine`, or this computer; with no
    /// `path`, the places a choice of one starts from.
    pub async fn folders(
        &self,
        machine: Option<Written>,
        path: Option<String>,
    ) -> Result<fastctx_types::focus::Folders, Error> {
        match self.ask(Call::Folders { machine, path }).await? {
            Reply::Folders(folders) => Ok(folders),
            other => Err(unexpected(other)),
        }
    }

    /// How `machine`, or this computer, is doing, for its details page; its
    /// lines' history only after `after`, when given.
    pub async fn stats(
        &self,
        machine: Option<fastctx_types::access::Written>,
        after: Option<jiff::Timestamp>,
        seen: Option<fastctx_types::stats::StatsSeen>,
    ) -> Result<fastctx_types::stats::MachineStats, Error> {
        match self.ask(Call::Stats { machine, after, seen }).await? {
            Reply::Stats(stats) => Ok(*stats),
            other => Err(unexpected(other)),
        }
    }

    /// What `machine`, or this computer, keeps free in memory for its
    /// agents; sets it first when `bytes` is given.
    pub async fn memory_room(
        &self,
        machine: Option<fastctx_types::access::Written>,
        bytes: Option<u64>,
    ) -> Result<u64, Error> {
        match self.ask(Call::MemoryRoom { machine, bytes }).await? {
            Reply::MemoryRoom { bytes } => Ok(bytes),
            other => Err(unexpected(other)),
        }
    }

    /// The HTTP proxy this computer reaches its hubs through: read, or with
    /// `change`, set to `url`, none clearing it.
    pub async fn hub_proxy(&self, url: Option<String>, change: bool) -> Result<Option<String>, Error> {
        match self.ask(Call::HubProxy { url, change }).await? {
            Reply::HubProxy { url } => Ok(url),
            other => Err(unexpected(other)),
        }
    }

    /// Renames this computer `to`; the names it had keep naming it.
    pub async fn rename_here(&self, to: MachineName) -> Result<(), Error> {
        match self.ask(Call::RenameHere { to }).await? {
            Reply::Done => Ok(()),
            other => Err(unexpected(other)),
        }
    }

    /// Every hub this computer is on, with what it sees of each.
    pub async fn accounts(&self) -> Result<AccountsReply, Error> {
        match self.ask(Call::Accounts).await? {
            Reply::Accounts(accounts) => Ok(accounts),
            other => Err(unexpected(other)),
        }
    }

    /// The networks said dissolved were seen: they are said no more.
    pub async fn seen_dissolved(&self) -> Result<(), Error> {
        match self.ask(Call::SeenDissolved).await? {
            Reply::Done => Ok(()),
            other => Err(unexpected(other)),
        }
    }

    /// Reads the selected account's machine changes from its hub, newest
    /// first. Pass the returned `next_before` to read older visible records.
    pub async fn machine_log(
        &self,
        hub: fastctx_types::HubName,
        account: fastctx_types::AccountName,
        before: Option<u64>,
    ) -> Result<fastctx_types::machine_log::Page, Error> {
        match self.ask(Call::MachineLog { hub, account, before }).await? {
            Reply::MachineLog(page) => Ok(page),
            other => Err(unexpected(other)),
        }
    }

    /// A verified hub-hosted installation line that registers a Linux
    /// server, or a Windows computer when `windows`, as a machine of
    /// `account`. Missing releases fail before the node makes a token.
    #[allow(clippy::too_many_arguments)]
    pub async fn server_command(
        &self,
        hub: Option<HubName>,
        account: AccountName,
        name: Option<MachineName>,
        expires: Timestamp,
        managed: bool,
        uses: Option<u32>,
        label: Option<String>,
        windows: bool,
        mac: bool,
    ) -> Result<String, Error> {
        match self.ask(Call::ServerCommand { hub, account, name, expires, managed, uses, label, windows, mac }).await? {
            Reply::ServerInvitation { command } => Ok(command),
            other => Err(unexpected(other)),
        }
    }

    /// Asks the hub named `hub`, or the one hub, for `request` of a move of
    /// it, as `account`, its super administrator.
    pub async fn hub_move(
        &self,
        hub: Option<HubName>,
        account: AccountName,
        request: fastctx_types::hub_move::HubMoveRequest,
    ) -> Result<fastctx_types::hub_move::HubMoveReply, Error> {
        match self.ask(Call::HubMove { hub, account, request }).await? {
            Reply::HubMove(reply) => Ok(reply),
            other => Err(unexpected(other)),
        }
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

fn machine_logins(reply: Reply) -> Result<fastctx_types::logins::MachineLogins, Error> {
    match reply {
        Reply::MachineAgentLogins(logins) => Ok(*logins),
        other => Err(unexpected(other)),
    }
}

fn done(reply: Reply) -> Result<(), Error> {
    match reply {
        Reply::Done => Ok(()),
        other => Err(unexpected(other)),
    }
}

fn logged_in(reply: Reply) -> Result<(HubName, MachineName, AccountName), Error> {
    match reply {
        Reply::LoggedIn { hub, name, account } => Ok((hub, name, account)),
        other => Err(unexpected(other)),
    }
}

fn unexpected(reply: Reply) -> Error {
    Error::Unexpected(format!("{reply:?}"))
}
