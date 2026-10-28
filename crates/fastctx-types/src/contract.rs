//! What answering each call takes, and how its caller waits for it
//! (`world-concurrency.md`, 等外面的事都有期限). Every call between machines
//! (`RemoteCall::contract`) and every call a client makes of its node
//! (`Call::on_leaving`) names its own in one match without a catch-all, so
//! no call is added without choosing.

use std::time::Duration;

use crate::calls::Call;
use crate::file_search::FileSearchRequest;
use crate::lines::Request as Lines;
use crate::mcp::McpCall;
use crate::remote::RemoteCall;
use crate::update::UpdateCall;

/// How long a stream whose machine speaks first, as a line source or a file
/// search opened through its own connection, is waited for to say it is
/// open: past that its caller gives it up, which ends it there too. Once
/// open it lasts as long as it lasts.
pub const OPENED_WITHIN: Duration = Duration::from_secs(60);

/// What a call between machines is, for whoever makes it and whoever
/// answers it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Contract {
    /// How long its caller waits for the answer from the moment the call is
    /// made, the wait for a connection to send it on included; none for work
    /// that goes on for as long as it goes on, kept alive by its line's
    /// heartbeat and its data connection's own, its bytes due coming within
    /// their own time (`fastctx_net::data::DUE_WITHIN`), and a stream that
    /// speaks first saying it is open within `OPENED_WITHIN`. Past it the
    /// caller gives the call up: whether what it asked was done is then not
    /// known. A copy asked of another machine (`RemoteCall::CopySend`) is
    /// taken on by the transfer that machine starts, and waited for as that is.
    pub within: Option<Duration>,
    /// Which places it takes where it is answered.
    pub pool: Pool,
    /// Whether the machine answering it stops at once when its caller gives
    /// it up. Dropping its future must stop the actual work as well as leave
    /// no state half done; blocking readers must finish while holding their place.
    pub cancellable: bool,
    /// Whether a node handing the World to its successor answers it before
    /// it goes; a stream instead goes on with the successor.
    pub held: bool,
}

/// The kinds of place a call takes on the machine answering it, each
/// bounded on its own: a machine busy with long work or many streams still
/// answers questions, and always takes a stop.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Pool {
    /// Read-only questions.
    Short,
    /// Changes other than direct human input.
    Change,
    /// Local draft edits, independent of submitting words or asking the host.
    Draft,
    /// Sending words and answering a host's question.
    Input,
    /// Stopping and interrupting, independent of every other kind of work.
    Stop,
    /// Work that runs a while before it answers: a script, a colleague.
    Long,
    /// A stream that stays open: a watch, a job's output, a copy, a read.
    Stream,
    /// Stopping, interrupting, and the facts that say a machine is there.
    Control,
    /// A call too large for one message while its bytes come, before what
    /// it is is known: bounded on its own, so slow uploads keep out no
    /// question.
    Receiving,
}

impl Contract {
    /// A question answered from what the machine holds.
    const QUERY: Self =
        Self { within: Some(Duration::from_secs(60)), pool: Pool::Short, cancellable: true, held: true };
    /// A question whose answer may wait on an agent's own program, which
    /// keeps its own deadlines of up to two minutes.
    const ASKS_AGENT: Self = Self { within: Some(Duration::from_secs(150)), ..Self::QUERY };
    /// A change, which may start or stop an agent's session first.
    const CHANGE: Self =
        Self { within: Some(Duration::from_secs(300)), pool: Pool::Change, cancellable: false, held: true };
    /// Work that answers once it is done, however long that takes.
    const LONG: Self = Self { within: None, pool: Pool::Long, cancellable: false, held: true };
    /// Running a script there, which answers within the longest a run
    /// waits (ten minutes, `fastctx_hands::run::LONGEST_WAIT`), the job going
    /// on there past that.
    const RUN: Self = Self { within: Some(Duration::from_secs(12 * 60)), ..Self::LONG };
    /// Starting a colleague there, which answers once it is started.
    const START: Self = Self { within: Some(Duration::from_secs(300)), ..Self::LONG };
    /// Bringing a release there and handing it to the renewal, which
    /// answers once the program is there, from GitHub or through the hub:
    /// waited for an hour at most, while the update goes on and is asked
    /// after again.
    const UPDATE: Self = Self { within: Some(Duration::from_secs(3600)), ..Self::LONG };
    /// A stream the successor of a node handing over takes on.
    const STREAM: Self = Self { within: None, pool: Pool::Stream, cancellable: false, held: false };
    /// A read over a data connection, held as it moves bytes.
    const READ: Self = Self { cancellable: true, held: true, ..Self::STREAM };
    /// A transfer that writes here, held as it moves bytes, and resumed
    /// rather than stopped halfway.
    const TRANSFER: Self = Self { held: true, ..Self::STREAM };
    /// Stopping and interrupting.
    const CONTROL: Self =
        Self { within: Some(Duration::from_secs(60)), pool: Pool::Control, cancellable: false, held: true };
}

impl RemoteCall {
    /// What this call is (`Contract`).
    pub fn contract(&self) -> Contract {
        match self {
            Self::Composer { request: crate::composer::Request::Read, .. } => Contract::QUERY,
            Self::Composer { request: crate::composer::Request::Mutate { mutation }, .. }
                if matches!(
                    mutation.change,
                    crate::composer::Change::Send { .. }
                        | crate::composer::Change::SendDocument { .. }
                        | crate::composer::Change::SendQueued { .. }
                ) =>
            {
                Contract { pool: Pool::Input, ..Contract::CHANGE }
            }
            Self::Composer { .. } => Contract { pool: Pool::Draft, ..Contract::CHANGE },
            Self::Talk { .. } | Self::Control { control: crate::session::control::Control::Answer { .. }, .. } => {
                Contract { pool: Pool::Input, ..Contract::CHANGE }
            }

            Self::Stop { .. } | Self::TurnInterrupted { .. } | Self::StopJob { .. } | Self::StopOn { .. } => {
                Contract { pool: Pool::Stop, ..Contract::CONTROL }
            }
            Self::Goal {
                request:
                    crate::goals::Request::Read | crate::goals::Request::History { .. } | crate::goals::Request::Past,
                ..
            } => Contract::QUERY,
            Self::GoalCommand { request: crate::goals::AgentRequest::Read, .. } => Contract::QUERY,
            Self::Goal { .. } | Self::GoalCommand { .. } => Contract::CHANGE,
            Self::SentinelControl {
                request: crate::sentinel_control::Request::List { .. } | crate::sentinel_control::Request::Help,
            }
            | Self::SentinelHome { request: crate::sentinel_control::HomeRequest::Inspect { .. } } => {
                Contract { within: Some(Duration::from_secs(90)), ..Contract::QUERY }
            }
            Self::SentinelHome { request: crate::sentinel_control::HomeRequest::Withdraw { .. } } => Contract::CONTROL,
            Self::SentinelControl { .. } | Self::SentinelHome { .. } => Contract::START,
            Self::SentinelSettings { change: None } => {
                Contract { within: Some(Duration::from_secs(180)), ..Contract::QUERY }
            }
            Self::SentinelSettings { change: Some(_) } => Contract::CHANGE,
            Self::SentinelKeys => Contract { within: Some(Duration::from_secs(30)), ..Contract::QUERY },
            Self::SentinelKey { .. } => Contract::CHANGE,
            Self::Mcp {
                mcp:
                    crate::mcp::MachineCall::Status { .. }
                    | crate::mcp::MachineCall::Found
                    | crate::mcp::MachineCall::Candidates
                    | crate::mcp::MachineCall::ImportRemote { .. },
            } => Contract::QUERY,
            Self::Mcp { mcp: crate::mcp::MachineCall::Keep { .. } | crate::mcp::MachineCall::Import { .. } } => {
                Contract::CHANGE
            }
            Self::Mcp { mcp: crate::mcp::MachineCall::Test { .. } } => Contract::ASKS_AGENT,
            Self::McpServe { .. } => Contract { cancellable: true, ..Contract::STREAM },
            Self::Collaboration { .. } => Contract { cancellable: true, ..Contract::ASKS_AGENT },
            Self::MemoryFolders { .. }
            | Self::Scratch { .. }
            | Self::AttachmentPlaces { .. }
            | Self::Who { .. }
            | Self::Since { .. }
            | Self::Folders { .. }
            | Self::Outline { .. }
            | Self::AgentLogins
            | Self::Canon { .. }
            | Self::Find { .. }
            | Self::Look { .. }
            | Self::ConversationDebt { .. }
            | Self::WaitingJobs { .. }
            | Self::ItemDetail { .. }
            | Self::AgentActivity { .. }
            | Self::Stats { .. }
            | Self::Book { .. }
            | Self::Interfaces
            | Self::OnReceipt { .. }
            | Self::Thumbnail { .. }
            | Self::AttachmentThumbnail { .. }
            | Self::Lines { request: Lines::Read { .. } | Lines::Close { .. } } => Contract::QUERY,
            // Start transfers ownership to an idle lease; Query and Close
            // join blocking work before giving their admission place back.
            Self::FindFiles {
                request:
                    FileSearchRequest::Start { .. } | FileSearchRequest::Query { .. } | FileSearchRequest::Close { .. },
            } => Contract { cancellable: false, ..Contract::QUERY },
            Self::Agents
            | Self::PreviewAgentChange { .. }
            | Self::ReadAgentLimits { .. }
            | Self::AgentUsage { .. }
            | Self::AgentCalls { .. }
            | Self::Offers { .. }
            | Self::Lines { request: Lines::Find { .. } } => Contract::ASKS_AGENT,
            Self::Plan { .. }
            | Self::EnterProject { .. }
            | Self::ReserveAttachment { .. }
            | Self::CommitAttachment { .. }
            | Self::PageAction { .. }
            | Self::Relay { .. }
            | Self::Forget { .. }
            | Self::Write { .. }
            | Self::ClaudeConsent { .. }
            | Self::Homes { .. }
            | Self::ScanAgents { .. }
            | Self::AddAgentLogin { .. }
            | Self::AgentFolder { .. }
            | Self::SetAgentTitle { .. }
            | Self::ChangeAgentLogin { .. }
            | Self::Open { .. }
            | Self::Control { .. }
            | Self::Manage { .. }
            | Self::Updates {
                update:
                    UpdateCall::State
                    | UpdateCall::Look
                    | UpdateCall::Set { .. }
                    | UpdateCall::Preferences { .. }
                    | UpdateCall::Progress { .. }
                    | UpdateCall::App { .. }
                    | UpdateCall::Fetch { .. }
                    | UpdateCall::Choose { .. }
                    | UpdateCall::Repair { .. },
            } => Contract::CHANGE,
            Self::UpdateWorld | Self::Updates { update: UpdateCall::Kernel { .. } } => Contract::UPDATE,
            Self::Run { .. } => Contract::RUN,
            Self::Ask { .. } => Contract::START,
            Self::On { .. }
            | Self::PipeOut { .. }
            | Self::Follow { .. }
            | Self::Watch { .. }
            | Self::CopySend { .. }
            | Self::Lines { request: Lines::Open { .. } | Lines::Changed { .. } }
            | Self::FindFiles { request: FileSearchRequest::Open { .. } } => Contract::STREAM,
            Self::ReadFile { .. }
            | Self::ReadAttachment { .. }
            | Self::ReadLines { .. }
            | Self::ReadLineNotes { .. }
            | Self::CopyFetch { .. } => Contract::READ,
            Self::Place { .. } | Self::Upload { .. } | Self::CopyReceive { .. } => Contract::TRANSFER,
            Self::AttachmentReferences { .. } | Self::Facts => Contract { cancellable: true, ..Contract::CONTROL },
            Self::WordsCame { .. } => Contract::CONTROL,
        }
    }
}

impl RemoteCall {
    /// The kind of this call as it is written (`watch`), read from the start
    /// of its writing alone, never the rest of it, which can be the user's
    /// words.
    pub fn kind(&self) -> String {
        /// How much of a call's writing is looked at for its kind.
        const HEAD: usize = 96;
        struct Head(Vec<u8>);
        impl std::io::Write for Head {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                let room = HEAD - self.0.len();
                if room == 0 {
                    return Err(std::io::Error::other("enough of the call is read"));
                }
                let taken = bytes.len().min(room);
                self.0.extend_from_slice(&bytes[..taken]);
                Ok(taken)
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut head = Head(Vec::with_capacity(HEAD));
        let _ = serde_json::to_writer(&mut head, self);
        const TAG: &[u8] = b"\"call\":\"";
        head.0
            .windows(TAG.len())
            .position(|window| window == TAG)
            .and_then(|at| {
                let rest = &head.0[at + TAG.len()..];
                let end = rest.iter().position(|byte| *byte == b'"')?;
                std::str::from_utf8(&rest[..end]).ok().map(str::to_string)
            })
            .unwrap_or_else(|| "call".to_string())
    }
}

/// What a node does with a call whose client left before its answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Leaving {
    /// Stops: it only reads or waits, and nobody is left to answer.
    Stop,
    /// Finishes: it changes something or joins blocking work before releasing resources.
    Finish,
}

impl Call {
    /// What its node does when its client leaves before the answer
    /// (`Leaving`).
    pub fn on_leaving(&self) -> Leaving {
        match self {
            Self::Composer { request: crate::composer::Request::Read, .. } => Leaving::Stop,
            Self::DoctorCalls | Self::PlanRead { .. } => Leaving::Stop,
            Self::Plan { .. } | Self::PlanWrite { .. } => Leaving::Finish,
            Self::AccountRecords {
                request: crate::account_records::Request::Read { .. } | crate::account_records::Request::List { .. },
                ..
            }
            | Self::WatchAccountRecords { .. } => Leaving::Stop,
            Self::AccountRecords {
                request: crate::account_records::Request::Write { .. } | crate::account_records::Request::Delete { .. },
                ..
            } => Leaving::Finish,
            Self::Memory(crate::memory::MemoryCall::List { .. } | crate::memory::MemoryCall::Read { .. }) => {
                Leaving::Stop
            }
            Self::Memory(crate::memory::MemoryCall::Save { .. })
            | Self::MemorySettings { enabled: Some(_), .. }
            | Self::Layer { layer: Some(_), .. } => Leaving::Finish,
            Self::WatchMemory { .. }
            | Self::MemorySettings { enabled: None, .. }
            | Self::Layer { layer: None, .. }
            | Self::Collaboration { .. } => Leaving::Stop,
            Self::Sentinel {
                request: crate::sentinel_control::Request::List { .. } | crate::sentinel_control::Request::Help,
                ..
            } => Leaving::Stop,
            Self::Sentinel { .. } => Leaving::Finish,
            Self::SentinelSettings { change: None, .. } => Leaving::Stop,
            Self::SentinelSettings { change: Some(_), .. } => Leaving::Finish,
            Self::SentinelKey { .. } => Leaving::Finish,
            Self::AttachmentPlaces { .. }
            | Self::McpRelay { .. }
            | Self::MachineLog { .. }
            | Self::CollabRead { .. }
            | Self::ProfileRead { .. }
            | Self::Mcp {
                mcp: McpCall::List | McpCall::Test { .. } | McpCall::Found { .. } | McpCall::Candidates { .. },
                ..
            }
            | Self::Features { .. }
            | Self::WatchFeatures { .. }
            | Self::WorldNow { .. }
            | Self::WatchWorld { .. }
            | Self::Wait { .. }
            | Self::Copy { .. }
            | Self::ReadFile { .. }
            | Self::Thumbnail { .. }
            | Self::AttachmentThumbnail { .. }
            | Self::ReadAttachment { .. }
            | Self::Lines { .. }
            | Self::ReadLines { .. }
            | Self::ReadLineNotes { .. }
            | Self::FindFiles { request: FileSearchRequest::Open { .. }, .. }
            | Self::Folders { .. }
            | Self::Stats { .. }
            | Self::Book { .. }
            | Self::WatchTools { .. }
            | Self::HoldHost { .. }
            | Self::Who { .. }
            | Self::Since { .. }
            | Self::Agents { .. }
            | Self::AgentLogins { .. }
            | Self::PreviewAgentChange { .. }
            | Self::ReadAgentLimits { .. }
            | Self::AgentUsage { .. }
            | Self::AgentCalls { .. }
            | Self::Look { .. }
            | Self::WatchMany
            | Self::Watch { .. }
            | Self::Offers { .. }
            | Self::ItemDetail { .. }
            | Self::AgentActivity { .. }
            | Self::Nodes { .. }
            | Self::Machines { .. } => Leaving::Stop,
            Self::FindFiles {
                request:
                    FileSearchRequest::Start { .. } | FileSearchRequest::Query { .. } | FileSearchRequest::Close { .. },
                ..
            }
            | Self::Goal { .. }
            | Self::GoalCommand { .. }
            | Self::Timer { .. }
            | Self::TurnEvidence { .. }
            | Self::CompactBetweenTurns { .. }
            | Self::CollabWrite { .. }
            | Self::ProfileWrite { .. }
            | Self::ProfileSubagents { .. }
            | Self::ClaudeSubagents { .. }
            | Self::Mcp { mcp: McpCall::Keep { .. } | McpCall::Remove { .. } | McpCall::Import { .. }, .. }
            | Self::SetFeature { .. }
            | Self::SetLanguage { .. }
            | Self::Reopened { .. }
            | Self::Composer { .. }
            | Self::Arrive { .. }
            | Self::ClaudeSessionDirectory { .. }
            | Self::Run { .. }
            | Self::StoreRunFacts { .. }
            | Self::RecordRunFailure { .. }
            | Self::On { .. }
            | Self::Write { .. }
            | Self::StopJob { .. }
            | Self::Ask { .. }
            | Self::Message { .. }
            | Self::Say { .. }
            | Self::Saw { .. }
            | Self::Stepped { .. }
            | Self::ClaudeConsent { .. }
            | Self::Homes { .. }
            | Self::ScanAgents { .. }
            | Self::AddAgentLogin { .. }
            | Self::AgentFolder { .. }
            | Self::SetAgentTitle { .. }
            | Self::ChangeAgentLogin { .. }
            | Self::Talk { .. }
            | Self::Upload { .. }
            | Self::Open { .. }
            | Self::Stop { .. }
            | Self::Control { .. }
            | Self::Data { .. }
            | Self::Update { .. }
            | Self::UpdateMachine { .. }
            | Self::MachineUpdates { .. }
            | Self::Manage { .. }
            | Self::Forget { .. }
            | Self::Attach { .. }
            | Self::Collect { .. }
            | Self::Taken { .. }
            | Self::Project { .. }
            | Self::Journal { .. }
            | Self::Page { .. }
            | Self::JobChanged { .. }
            | Self::Login { .. }
            | Self::PeekToken { .. }
            | Self::LoginWithToken { .. }
            | Self::OpenAccount { .. }
            | Self::Account { .. }
            | Self::NetworkUpdates { .. }
            | Self::Accounts { .. }
            | Self::RenameHere { .. }
            | Self::HubProxy { .. }
            | Self::ServerCommand { .. }
            | Self::HubMove { .. }
            | Self::HubArrived { .. }
            | Self::HandOver { .. }
            | Self::SeenDissolved { .. }
            | Self::UninstallPrepare { .. }
            | Self::BringIn { .. } => Leaving::Finish,
        }
    }
}
