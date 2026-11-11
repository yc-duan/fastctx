//! How calls are meant to answer, independently of their present implementation.

use crate::{calls::Call, remote::RemoteCall};

/// The work a call is allowed to do before it answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Response {
    /// Answers from bounded local work, such as a short transaction or a page read.
    Answer,
    /// Records the work to do and acknowledges it; execution follows afterwards.
    Submit,
    /// A live transfer or wait whose lifetime is not that of a short answer.
    /// This timing classification does not change the call's transport or admission.
    Stream,
}

impl RemoteCall {
    /// How this call is meant to answer, even while its handler is being adapted.
    pub fn response(&self) -> Response {
        match self {
            Self::AgentActivity { .. }
            | Self::AgentCalls { .. }
            | Self::AgentLogins
            | Self::AgentUsage { .. }
            | Self::Agents
            | Self::AttachmentPlaces { .. }
            | Self::AttachmentReferences { .. }
            | Self::AttachmentThumbnail { .. }
            | Self::Book { .. }
            | Self::Canon { .. }
            | Self::ChangeAgentModels { .. }
            | Self::ClaudeConsent { .. }
            | Self::Collaboration { .. }
            | Self::CommitAttachment { .. }
            | Self::ConversationDebt { .. }
            | Self::Facts
            | Self::Find { .. }
            | Self::Folders { .. }
            | Self::Inspect { .. }
            | Self::Interfaces
            | Self::ItemDetail { .. }
            | Self::Look { .. }
            | Self::MemoryFolders { .. }
            | Self::MemoryRoom { .. }
            | Self::Offers { .. }
            | Self::OnReceipt { .. }
            | Self::Outline { .. }
            | Self::PreviewAgentChange { .. }
            | Self::ReserveAttachment { .. }
            | Self::Scratch { .. }
            | Self::SentinelKeys
            | Self::SetAgentTitle { .. }
            | Self::Since { .. }
            | Self::Stats { .. }
            | Self::Stop { .. }
            | Self::Thumbnail { .. }
            | Self::WaitingJobs { .. }
            | Self::Who { .. }
            | Self::WordsCame { .. }
            | Self::Write { .. } => Response::Answer,
            Self::AddAgentLogin { .. }
            | Self::Ask { .. }
            | Self::ChangeAgentLogin { .. }
            | Self::EnterProject { .. }
            | Self::Forget { .. }
            | Self::Manage { .. }
            | Self::Open { .. }
            | Self::Plan { .. }
            | Self::ReadAgentLimits { .. }
            | Self::Run { .. }
            | Self::SentinelKey { .. }
            | Self::SentinelTry
            | Self::SetAgentResetInPlace { .. }
            | Self::StopJob { .. }
            | Self::StopOn { .. }
            | Self::Talk { .. }
            | Self::UpdateWorld
            | Self::WorkingCopy { .. } => Response::Submit,
            Self::CopyFetch { .. }
            | Self::CopyReceive { .. }
            | Self::CopySend { .. }
            | Self::Follow { .. }
            | Self::McpServe { .. }
            | Self::On { .. }
            | Self::PipeOut { .. }
            | Self::Place { .. }
            | Self::ReadAttachment { .. }
            | Self::ReadFile { .. }
            | Self::ReadLineNotes { .. }
            | Self::ReadLines { .. }
            | Self::Upload { .. }
            | Self::Watch { .. } => Response::Stream,

            Self::Composer { request, .. } => composer(request),
            Self::Control { control: request, .. } => control(request),
            Self::Goal { request, .. } => goal(request),
            Self::GoalCommand { request, .. } => goal_command(request),
            Self::SentinelControl { request } => sentinel(request, Response::Answer),
            Self::SentinelHome { request } => sentinel_home(request),
            Self::SentinelSettings { change } => match change {
                None => Response::Answer,
                Some(_) => Response::Submit,
            },
            Self::Mcp { mcp } => machine_mcp(mcp),
            Self::PageAction { action, .. } => page_action(action),
            Self::TurnInterrupted { dispatch, .. } => match dispatch {
                None => Response::Answer,
                Some(_) => Response::Submit,
            },
            Self::Lines { request } => lines(request),
            Self::Timeline { request } => timeline(request),
            Self::FindFiles { request } => find_files(request),
            Self::Homes { change } => homes(change),
            Self::ScanAgents { stop } => scan(*stop),
            Self::AgentFolder { change, .. } => folder(change),
            Self::Updates { update } => update_response(update, Response::Answer),
            Self::Relay { made } => relay(made),
        }
    }
}

impl Call {
    /// How this call is meant to answer, even while its handler is being adapted.
    pub fn response(&self) -> Response {
        match self {
            Self::Accounts
            | Self::AgentActivity { .. }
            | Self::AgentCalls { .. }
            | Self::AgentLogins { .. }
            | Self::AgentUsage { .. }
            | Self::Agents { .. }
            | Self::Arrive { .. }
            | Self::AttachmentPlaces { .. }
            | Self::AttachmentThumbnail { .. }
            | Self::Book { .. }
            | Self::BringIn { .. }
            | Self::ChangeAgentModels { .. }
            | Self::ClaudeConsent { .. }
            | Self::ClaudeContext { .. }
            | Self::ClaudeSessionDirectory { .. }
            | Self::CollabRead { .. }
            | Self::Collaboration { .. }
            | Self::DoctorCalls
            | Self::DoctorReadings
            | Self::Features
            | Self::Folders { .. }
            | Self::HandOver
            | Self::HistorySwitchRead { .. }
            | Self::HubProxy { .. }
            | Self::ItemDetail { .. }
            | Self::Journal(_)
            | Self::JobChanged { .. }
            | Self::Layer { .. }
            | Self::Look { .. }
            | Self::MachineLog { .. }
            | Self::Machines { .. }
            | Self::Memory(_)
            | Self::MemoryRoom { .. }
            | Self::MemorySettings { .. }
            | Self::Nodes
            | Self::Offers { .. }
            | Self::PlanRead { .. }
            | Self::PeekToken { .. }
            | Self::PreviewAgentChange { .. }
            | Self::ProfileRead { .. }
            | Self::RecordRunFailure { .. }
            | Self::Saw { .. }
            | Self::Say { .. }
            | Self::SeenDissolved
            | Self::SetAgentTitle { .. }
            | Self::SetFeature { .. }
            | Self::SetLanguage { .. }
            | Self::Since { .. }
            | Self::Skill { .. }
            | Self::Stats { .. }
            | Self::Stepped { .. }
            | Self::Stop { .. }
            | Self::StoreRunFacts { .. }
            | Self::Taken { .. }
            | Self::Thumbnail { .. }
            | Self::Timer { .. }
            | Self::TurnEvidence { .. }
            | Self::WatchMemory { .. }
            | Self::Who { .. }
            | Self::WorldNow { .. } => Response::Answer,
            Self::AddAgentLogin { .. }
            | Self::Ask { .. }
            | Self::ChangeAgentLogin { .. }
            | Self::CollabWrite { .. }
            | Self::CompactBetweenTurns { .. }
            | Self::Forget { .. }
            | Self::HistorySwitchWrite { .. }
            | Self::HubArrived { .. }
            | Self::HubMove { .. }
            | Self::Login { .. }
            | Self::LoginWithToken { .. }
            | Self::Manage { .. }
            | Self::Message { .. }
            | Self::NetworkUpdates { .. }
            | Self::Open { .. }
            | Self::OpenAccount { .. }
            | Self::Plan { .. }
            | Self::PlanWrite { .. }
            | Self::PrepareCopies { .. }
            | Self::ProfileWrite { .. }
            | Self::ReadAgentLimits { .. }
            | Self::RenameHere { .. }
            | Self::Run { .. }
            | Self::SentinelKey { .. }
            | Self::SentinelTry { .. }
            | Self::ServerCommand { .. }
            | Self::SetAgentResetInPlace { .. }
            | Self::StopJob { .. }
            | Self::Talk { .. }
            | Self::UninstallPrepare
            | Self::UpdateMachine { .. }
            | Self::Write { .. } => Response::Submit,
            Self::Attach { .. }
            | Self::ClaudeSubagents { .. }
            | Self::Collect { .. }
            | Self::Copy { .. }
            | Self::HoldHost { .. }
            | Self::McpRelay { .. }
            | Self::On { .. }
            | Self::ReadAttachment { .. }
            | Self::ReadFile { .. }
            | Self::ReadLineNotes { .. }
            | Self::ReadLines { .. }
            | Self::Reopened { .. }
            | Self::Upload { .. }
            | Self::Wait { .. }
            | Self::Watch { .. }
            | Self::WatchAccountRecords { .. }
            | Self::WatchFeatures
            | Self::WatchHistory { .. }
            | Self::WatchMany
            | Self::WatchTools { .. }
            | Self::WatchWorld { .. } => Response::Stream,

            Self::Account { change, .. } => match change {
                crate::accounts::AccountChange::Seen { .. } => Response::Answer,
                _ => Response::Submit,
            },
            Self::Composer { request, .. } => composer(request),
            Self::Control { control: request, .. } => control(request),
            Self::Goal { request, .. } => goal(request),
            Self::GoalCommand { request, .. } => goal_command(request),
            Self::Sentinel { request, .. } => sentinel(request, Response::Submit),
            Self::SentinelSettings { change, .. } => match change {
                None => Response::Answer,
                Some(_) => Response::Submit,
            },
            Self::Mcp { mcp, .. } => local_mcp(mcp),
            Self::Lines { request, .. } => lines(request),
            Self::FindFiles { request, .. } => find_files(request),
            Self::Homes { change, .. } => homes(change),
            Self::ScanAgents { stop, .. } => scan(*stop),
            Self::AgentFolder { change, .. } => folder(change),
            Self::Update(update) | Self::MachineUpdates { update, .. } => update_response(update, Response::Submit),
            Self::SkillFiles { request, .. } => {
                use crate::skill_files::Request;
                match request {
                    Request::Limits | Request::List | Request::Read { .. } | Request::ReadFile { .. } => {
                        Response::Answer
                    }
                    Request::Write { .. } | Request::Edit { .. } | Request::Delete { .. } => Response::Submit,
                }
            }
            Self::AccountRecords { request, .. } => {
                use crate::account_records::Request;
                match request {
                    Request::Read { .. } | Request::List { .. } => Response::Answer,
                    Request::Write { .. } | Request::Delete { .. } => Response::Submit,
                }
            }
            Self::History(request) => {
                use crate::history::HistoryCall;
                match request {
                    HistoryCall::List { .. } | HistoryCall::Read { .. } => Response::Answer,
                    HistoryCall::Save { .. } | HistoryCall::Retire { .. } | HistoryCall::Restore { .. } => {
                        Response::Submit
                    }
                }
            }
            Self::Timeline { request, .. } => timeline(request),
            // It waits for a change, as a timeline's `Moved` does.
            Self::Topics { .. } => Response::Stream,
            Self::Data(request) => {
                use crate::data::DataCall;
                match request {
                    DataCall::List | DataCall::OutputDays { .. } => Response::Answer,
                    DataCall::Delete { .. } | DataCall::ClearCache => Response::Submit,
                }
            }
            Self::ProfileSubagents { operation, .. } => {
                use crate::profile::SubagentOperation;
                match operation {
                    SubagentOperation::List => Response::Answer,
                    SubagentOperation::Save { .. }
                    | SubagentOperation::Delete { .. }
                    | SubagentOperation::Instructions { .. }
                    | SubagentOperation::Reset { .. } => Response::Submit,
                }
            }
            Self::Project(request) => {
                use crate::calls::ProjectCall;
                match request {
                    ProjectCall::List | ProjectCall::Show { .. } | ProjectCall::Map { .. } => Response::Answer,
                    ProjectCall::New { .. } | ProjectCall::Edit { .. } | ProjectCall::Enter { .. } => Response::Submit,
                }
            }
            Self::Page(request) => {
                use crate::pages::PageCall;
                match request {
                    PageCall::Read { .. } | PageCall::List { .. } | PageCall::History { .. } => Response::Answer,
                    PageCall::Publish { .. }
                    | PageCall::Withdraw { .. }
                    | PageCall::Restore { .. }
                    | PageCall::Consent { .. } => Response::Submit,
                    PageCall::Act { action, .. } => page_action(action),
                }
            }
        }
    }
}

fn composer(request: &crate::composer::Request) -> Response {
    use crate::composer::{Change, Request};
    match request {
        Request::Read | Request::Sync { .. } => Response::Answer,
        Request::Mutate { mutation } => match &mutation.change {
            Change::Send { .. } | Change::SendDocument { .. } | Change::SendQueued { .. } => Response::Submit,
            Change::Edit { .. }
            | Change::Attach { .. }
            | Change::Detach { .. }
            | Change::FilesOn { .. }
            | Change::Editing { .. }
            | Change::Delete { .. }
            | Change::Move { .. }
            | Change::Withdraw { .. } => Response::Answer,
        },
    }
}

fn control(request: &crate::session::control::Control) -> Response {
    use crate::session::control::Control;
    match request {
        // The request names a resident, not its host. Some hosts acknowledge an
        // answer only after consuming its durable receipt.
        Control::Answer { .. }
        | Control::Command { .. }
        | Control::Compact
        | Control::Model { .. }
        | Control::Rename { .. }
        | Control::Fork { .. }
        | Control::Rewind { .. } => Response::Submit,
        Control::CancelControl { .. } | Control::Agents | Control::AgentMessages { .. } => Response::Answer,
    }
}

fn goal(request: &crate::goals::Request) -> Response {
    use crate::goals::Request;
    match request {
        Request::Read | Request::History { .. } | Request::Past | Request::Mark { .. } => Response::Answer,
        Request::Change { .. } => Response::Submit,
    }
}

fn goal_command(request: &crate::goals::AgentRequest) -> Response {
    use crate::goals::AgentRequest;
    match request {
        AgentRequest::Read => Response::Answer,
        AgentRequest::Mark { .. } | AgentRequest::ControlColleague { .. } => Response::Submit,
    }
}

fn sentinel(request: &crate::sentinel_control::Request, create: Response) -> Response {
    use crate::sentinel_control::Request;
    match request {
        Request::Create { .. } => create,
        Request::Stop { .. } => Response::Submit,
        Request::List { .. } | Request::Help | Request::Record { .. } | Request::Overview => Response::Answer,
        Request::Conversation { request, .. } => match request.as_ref() {
            Request::Stop { .. } => Response::Submit,
            Request::List { .. }
            | Request::Record { .. }
            | Request::Overview
            | Request::Create { .. }
            | Request::Help
            | Request::Conversation { .. } => Response::Answer,
        },
    }
}

fn sentinel_home(request: &crate::sentinel_control::HomeRequest) -> Response {
    use crate::sentinel_control::HomeRequest;
    match request {
        HomeRequest::Begin { .. } | HomeRequest::Judge { .. } | HomeRequest::Withdraw { .. } => Response::Submit,
        HomeRequest::Inspect { .. } | HomeRequest::Record { .. } | HomeRequest::Overview => Response::Answer,
        HomeRequest::Conversation { request, .. } => match request.as_ref() {
            HomeRequest::Withdraw { .. } => Response::Submit,
            HomeRequest::Inspect { .. }
            | HomeRequest::Record { .. }
            | HomeRequest::Overview
            | HomeRequest::Begin { .. }
            | HomeRequest::Judge { .. }
            | HomeRequest::Conversation { .. } => Response::Answer,
        },
    }
}

fn machine_mcp(request: &crate::mcp::MachineCall) -> Response {
    use crate::mcp::MachineCall;
    match request {
        MachineCall::Status { .. }
        | MachineCall::Found
        | MachineCall::Candidates
        | MachineCall::ImportRemote { .. } => Response::Answer,
        MachineCall::Keep { .. } | MachineCall::Import { .. } | MachineCall::Test { .. } => Response::Submit,
    }
}

fn local_mcp(request: &crate::mcp::McpCall) -> Response {
    use crate::mcp::McpCall;
    match request {
        McpCall::List | McpCall::Found { .. } | McpCall::Candidates { .. } => Response::Answer,
        McpCall::Keep { .. } | McpCall::Remove { .. } | McpCall::Import { .. } | McpCall::Test { .. } => {
            Response::Submit
        }
    }
}

fn page_action(request: &crate::pages::PageInvocation) -> Response {
    use crate::pages::PageInvocation;
    match request {
        PageInvocation::Run { .. } | PageInvocation::Say { .. } => Response::Submit,
        PageInvocation::Journal { .. } => Response::Answer,
    }
}

/// A page and a piece of an object are read; waiting for the timeline to
/// move lasts until it does.
fn timeline(request: &crate::timeline_pages::TimelineAsk) -> Response {
    use crate::timeline_pages::TimelineAsk;
    match request {
        TimelineAsk::Moved { .. } => Response::Stream,
        TimelineAsk::Page { .. } | TimelineAsk::Object { .. } | TimelineAsk::Changes { .. } => Response::Answer,
    }
}

fn lines(request: &crate::lines::Request) -> Response {
    use crate::lines::Request;
    match request {
        Request::Open { .. } | Request::Changed { .. } => Response::Stream,
        Request::Read { .. } | Request::Close { .. } | Request::Find { .. } => Response::Answer,
    }
}

fn find_files(request: &crate::file_search::FileSearchRequest) -> Response {
    use crate::file_search::FileSearchRequest;
    match request {
        FileSearchRequest::Open { .. } => Response::Stream,
        FileSearchRequest::Start { .. } | FileSearchRequest::Query { .. } | FileSearchRequest::Close { .. } => {
            Response::Answer
        }
    }
}

fn homes(change: &crate::agents::HomeChange) -> Response {
    use crate::agents::HomeChange;
    match change {
        HomeChange::Add { .. } => Response::Submit,
        HomeChange::Hide { .. } | HomeChange::Show { .. } => Response::Answer,
    }
}

fn scan(stop: bool) -> Response {
    if stop { Response::Answer } else { Response::Submit }
}

fn folder(change: &crate::logins::FolderChange) -> Response {
    use crate::logins::FolderChange;
    match change {
        FolderChange::Choose => Response::Submit,
        FolderChange::Stop | FolderChange::Forget => Response::Answer,
    }
}

fn update_response(request: &crate::update::UpdateCall, fetch: Response) -> Response {
    use crate::update::UpdateCall;
    match request {
        UpdateCall::State
        | UpdateCall::Set { .. }
        | UpdateCall::Preferences { .. }
        | UpdateCall::App { .. }
        | UpdateCall::Progress { .. } => Response::Answer,
        UpdateCall::Fetch { .. } => fetch,
        UpdateCall::Look | UpdateCall::Kernel { .. } | UpdateCall::Choose { .. } | UpdateCall::Repair { .. } => {
            Response::Submit
        }
    }
}

fn relay(made: &serde_json::Value) -> Response {
    // Devices relay only this whitelist. Other payloads are rejected, including
    // unknown tags; inspecting those tags does not deserialize another Call.
    let text = |key| made.get(key).and_then(serde_json::Value::as_str);
    let action = made.get("action").and_then(|action| action.get("kind")).and_then(serde_json::Value::as_str);
    match (text("call"), text("do"), action) {
        (Some("page"), Some("consent" | "withdraw" | "restore"), _)
        | (Some("page"), Some("act"), Some("run" | "say")) => Response::Submit,
        _ => Response::Answer,
    }
}
