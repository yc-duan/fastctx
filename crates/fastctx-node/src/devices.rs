//! What a device may ask of this machine. A device, such as a phone, is a
//! member of a hub that carries the user's own words and looks and runs
//! nothing (`fastctx_net::directory::Kind`). What concerns one resident it
//! asks of that resident's machine itself, as a machine does for its user:
//! so the words are known to have been typed on the device, and a watch
//! comes straight from where the conversation lives; a file the user reads,
//! it reads from the machine that has it. Who is here and what happened, it
//! asks of each machine its accounts may use itself. What only the
//! projects' ledgers know, a project's record, its journals and its pages,
//! it asks one machine to answer from its own copy, for the account it
//! speaks for: that machine answers about a project of that account only,
//! and what the call reaches on other machines, the account must reach
//! too. A device never runs, writes, copies, asks a colleague or sends a
//! letter: those are a machine's and its residents' own.

use std::sync::Arc;

use fastctx_net::directory::View;
use fastctx_types::access::Purpose;
use fastctx_types::calls::Call;
use fastctx_types::ipc::{Failure, Peer};
use fastctx_types::management::Management;
use fastctx_types::pages::{PageCall, PageInvocation};
use fastctx_types::remote::{RemoteCall, RemoteReply, RemoteResult};
use fastctx_types::{MachineName, ProjectCall, Reply, TownName};

use crate::Node;
use crate::admission::Asker;

/// Refuses a call `from` may not make: a device asks only for the user's
/// own calls, and only a device has its calls made for it.
pub fn admit(view: &View, from: &MachineName, call: &RemoteCall) -> Result<(), Failure> {
    let device = view.member(from).is_some_and(|member| !member.is_machine());
    let users_own = matches!(
        call,
        RemoteCall::Collaboration { .. }
            | RemoteCall::SentinelSettings { .. }
            | RemoteCall::SentinelControl { .. }
            // The user's own key, handed straight to the sentinel home.
            | RemoteCall::SentinelKey { .. }
            | RemoteCall::Mcp { .. }
            | RemoteCall::Talk { .. }
            | RemoteCall::Goal { .. }
            | RemoteCall::Composer { .. }
            | RemoteCall::Open { .. }
            | RemoteCall::Stop { .. }
            // Stopping a command, as the user asks, on the machine it runs on.
            | RemoteCall::StopJob { .. }
            | RemoteCall::Control { .. }
            | RemoteCall::Manage {
                action: Management::Archive
                    | Management::Restore
                    | Management::Sleep
                    | Management::Wake
                    | Management::Move { .. }
                    | Management::Focus { .. }
                    | Management::Mcp { .. },
                ..
            }
            | RemoteCall::Offers { .. }
            | RemoteCall::ItemDetail { .. }
            | RemoteCall::AgentActivity { .. }
            | RemoteCall::Watch { .. }
            // Reading the user's own files, from the machine that has them,
            // and a file the user attaches to their words, kept in that
            // agent's own place: byte streams, which no relay carries.
            | RemoteCall::ReadFile { .. }
            | RemoteCall::Thumbnail { .. }
            | RemoteCall::AttachmentThumbnail { .. }
            | RemoteCall::ReadAttachment { .. }
            | RemoteCall::Lines { .. }
            | RemoteCall::ReadLines { .. }
            | RemoteCall::ReadLineNotes { .. }
            | RemoteCall::Upload { .. }
            // Names of the user's files for an `@` in their words: the
            // machine that has them limits the search to the conversation's
            // places, and reads no contents.
            | RemoteCall::FindFiles { .. }
            // The names of the user's folders, for pointing a conversation
            // at some: names only, never contents.
            | RemoteCall::Folders { .. }
            // How a machine of the user's is doing, for its details page.
            | RemoteCall::Stats { .. }
            // A machine of the user's updated, from its details page.
            | RemoteCall::UpdateWorld
            | RemoteCall::Updates { .. }
            // The user's consent to Claude Code's bypass-permissions mode,
            // which only the user gives, on the machine it is about.
            | RemoteCall::ClaudeConsent { .. }
            | RemoteCall::Relay { .. }
            // The agents' logins of the user's own machines: scanned, taken
            // up, changed and read only for the user, on the machine itself.
            | RemoteCall::AgentLogins
            | RemoteCall::ScanAgents { .. }
            | RemoteCall::AddAgentLogin { .. }
            | RemoteCall::AgentFolder { .. }
            | RemoteCall::SetAgentTitle { .. }
            | RemoteCall::PreviewAgentChange { .. }
            | RemoteCall::ChangeAgentLogin { .. }
            | RemoteCall::ReadAgentLimits { .. }
            | RemoteCall::AgentUsage { .. }
            | RemoteCall::AgentCalls { .. }
            // What is there, as the account may see it.
            | RemoteCall::Agents
            | RemoteCall::Look { .. }
            | RemoteCall::Find { .. }
            | RemoteCall::Who { .. }
            | RemoteCall::Since { .. }
            | RemoteCall::Facts
    );
    if device && !users_own {
        return Err(Failure::not_run(format!(
            "{from} is a device; a device asks for the user's own calls, and runs, writes and copies nothing"
        )));
    }
    if !device && matches!(call, RemoteCall::Relay { .. }) {
        return Err(Failure::not_run(format!("{from} is a machine; only a device has its calls made for it")));
    }
    Ok(())
}

/// Makes a device's call about a project, as for a user at this machine,
/// for the account it speaks for, when that account may use the project.
pub async fn relay(node: &Arc<Node>, asker: &Asker, call: serde_json::Value) -> RemoteResult {
    let from = &asker.machine;
    let here = node.machine();
    let call: Call = serde_json::from_value(call)
        .map_err(|error| Failure::new(format!("{from} asked an unreadable call: {error}")))?;
    let refuse = |why: String| Failure::not_run(format!("{here} refused {from}: {why}"));
    let link = node.link_of(&asker.hub).ok_or_else(|| Failure::new("the link that brought the call is gone"))?;
    let view = link.view();
    let account = &asker.account;
    // The project the call is about: `None` for the projects as a whole,
    // `Some(None)` for "the caller's", which a device has none of.
    let town: Option<Option<&TownName>> = match &call {
        Call::Project(ProjectCall::List) => None,
        Call::Project(ProjectCall::Show { town } | ProjectCall::Map { town } | ProjectCall::Edit { town, .. }) => {
            Some(town.as_ref())
        }
        Call::Project(ProjectCall::New { .. }) => {
            return Err(refuse("a project is opened in an account from a computer logged in to it".into()));
        }
        Call::Journal(_) => return Err(Failure::not_run(fastctx_types::memory::LEGACY)),
        Call::Page(
            PageCall::Read { town, .. }
            | PageCall::List { town }
            | PageCall::History { town, .. }
            | PageCall::Consent { town, .. }
            | PageCall::Act { town, .. }
            | PageCall::Withdraw { town, .. }
            | PageCall::Restore { town, .. },
        ) => Some(town.as_ref()),
        _ => {
            return Err(Failure::new(
                "a device asks this of the machine it concerns itself, or not at all: only calls about a project are made for it",
            ));
        }
    };
    let may_use =
        |town: &TownName| crate::ledger::names::town_id(node, town).is_some_and(|id| view.in_project(account, &id));
    match town {
        Some(Some(town)) if !may_use(town) => {
            return Err(refuse(format!("the account {account} may not use the project {town}")));
        }
        Some(None) => return Err(Failure::new(format!("{from} named no project; a device names the one it means"))),
        _ => {}
    }
    // What the call reaches on other machines, the account must reach too:
    // this machine reaches them as itself.
    if let Call::Page(PageCall::Act { action, .. }) = &call {
        let (target, purpose) = match action {
            PageInvocation::Run { machine, .. } => (machine, Purpose::Hands),
            PageInvocation::Say { to, .. } => (&to.machine, Purpose::Words),
            PageInvocation::Journal { .. } => return Err(Failure::not_run(fastctx_types::memory::LEGACY)),
        };
        view.admits(from, account, target, purpose).map_err(refuse)?;
    }
    let user = Peer::Command { resident: None, session: None };
    let reply = match call {
        Call::Project(ProjectCall::Map { town: Some(town) }) => {
            let acting = fastctx_types::access::Acting::one(asker.hub.clone(), account.clone());
            let map = crate::town::map::view(node, &town, &acting).await?;
            Reply::Map { map: Box::new(map) }
        }
        Call::Page(mut call) => {
            if call.account().is_some_and(|selected| selected != account) {
                return Err(Failure::refused("the page account differs from the authenticated device account"));
            }
            call.for_account(account.clone());
            crate::pages::call(node, &crate::town::caller::Caller::User, from, call).await?
        }
        call => match crate::server::handle(node, &user, call).await? {
            Reply::Towns { towns } => {
                Reply::Towns { towns: towns.into_iter().filter(|summary| may_use(&summary.town)).collect() }
            }
            // The canon lives on a machine of its own, which this machine
            // read as itself: the device sees it only if it may read there.
            Reply::Map { mut map } => {
                if let Some(canon) = &map.record.canon
                    && let Err(refused) = view.admits(from, account, &canon.machine, Purpose::Hands)
                {
                    map.canon = Some(Err(refused));
                }
                Reply::Map { map }
            }
            reply => reply,
        },
    };
    let reply = serde_json::to_value(reply).map_err(|error| Failure::new(format!("writing the answer: {error}")))?;
    Ok(RemoteReply::Relayed { answer: reply })
}
