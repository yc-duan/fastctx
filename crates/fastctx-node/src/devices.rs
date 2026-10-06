//! What a device may ask of this machine. A device, such as a phone, is a
//! member of the World that carries the user's own words and looks and runs
//! nothing (`fastctx_net::trust::Kind`). What concerns one resident it asks
//! of that resident's machine itself, as a machine does for its user: so
//! the words are known to have been typed on the device, and a watch comes
//! straight from where the conversation lives; a file the user reads, it
//! reads from the machine that has it. Who is here and what happened, it
//! asks of each machine its scope covers itself, each answering what that
//! scope may see. What only the projects' ledgers know, a project's record,
//! its journals and its pages, it asks one machine to answer from its own
//! copy, within the device's scope: that machine makes the call for it as
//! for a user at that machine, about one project the scope includes, and
//! makes no call to another machine on its behalf that the device could not
//! make itself. A device never runs, writes, copies, asks a colleague or
//! sends a letter: those are a machine's and its residents' own. No machine invites
//! or revokes for it either: a machine's invitation is a machine's word,
//! so a lost phone could bring a stranger in through one; a device revokes
//! only itself, over its own link, when it leaves.

use std::sync::Arc;

use fastctx_net::scope::{Action, Denied};
use fastctx_net::trust::Member;
use fastctx_types::calls::Call;
use fastctx_types::ipc::{Failure, Peer};
use fastctx_types::management::Management;
use fastctx_types::pages::{PageCall, PageInvocation};
use fastctx_types::remote::{RemoteCall, RemoteReply, RemoteResult};
use fastctx_types::{JournalCall, ProjectCall, Reply, TownName};

use crate::Node;

/// Refuses a call `member` may not make: a device asks only for the user's
/// own calls, and only a device has its calls made for it.
pub fn admit(member: &Member, call: &RemoteCall) -> Result<(), Failure> {
    let from = &member.name;
    let device = !member.kind.is_machine();
    let users_own = matches!(
        call,
        RemoteCall::Talk { .. }
            | RemoteCall::Open { .. }
            | RemoteCall::Stop { .. }
            | RemoteCall::Control { .. }
            | RemoteCall::Manage {
                action: Management::Remove | Management::Sleep | Management::Wake | Management::Move { .. },
                ..
            }
            | RemoteCall::Offers { .. }
            | RemoteCall::Watch { .. }
            // Reading the user's own files, from the machine that has them,
            // and a file the user attaches to their words, kept in that
            // agent's own place: byte streams, which no relay carries.
            | RemoteCall::ReadFile { .. }
            | RemoteCall::Upload { .. }
            // Names of the user's files for an `@` in their words: the
            // machine that has them limits the search to the conversation's
            // places, and reads no contents.
            | RemoteCall::FindFiles { .. }
            // The user's consent to Claude Code's bypass-permissions mode,
            // which only the user gives, on the machine it is about.
            | RemoteCall::ClaudeConsent { .. }
            | RemoteCall::Relay { .. }
    );
    if device && !users_own {
        return Err(Failure::new(format!(
            "{from} is a device; a device asks for the user's own calls, and runs, writes and copies nothing"
        )));
    }
    if !device && matches!(call, RemoteCall::Relay { .. }) {
        return Err(Failure::new(format!("{from} is a machine; only a device has its calls made for it")));
    }
    Ok(())
}

/// Makes a device's call about a project, as for a user at this machine,
/// when the device's scope covers it.
pub async fn relay(node: &Arc<Node>, member: &Member, call: serde_json::Value) -> RemoteResult {
    let from = &member.name;
    let here = node.machine();
    let call: Call = serde_json::from_value(call)
        .map_err(|error| Failure::new(format!("{from} asked an unreadable call: {error}")))?;
    let refuse = |denied: Denied| Failure::new(format!("{here} refused {from}: {denied}"));
    // The action the call is, and the project it is about: `None` for the
    // projects as a whole, `Some(None)` for "the caller's", which a device
    // has none of.
    let (action, town): (Action, Option<Option<&TownName>>) = match &call {
        Call::Project(ProjectCall::List) => (Action::W, None),
        Call::Project(ProjectCall::Show { town } | ProjectCall::Map { town }) => (Action::W, Some(town.as_ref())),
        Call::Project(ProjectCall::New { town, .. }) => (Action::Write, Some(Some(town))),
        Call::Project(ProjectCall::Edit { town, .. }) => (Action::Write, Some(town.as_ref())),
        Call::Journal(JournalCall::Read { town, .. } | JournalCall::List { town }) => (Action::W, Some(town.as_ref())),
        Call::Journal(JournalCall::Add { town, .. } | JournalCall::Describe { town, .. }) => {
            (Action::Write, Some(town.as_ref()))
        }
        Call::Page(PageCall::Read { town, .. } | PageCall::List { town } | PageCall::History { town, .. }) => {
            (Action::W, Some(town.as_ref()))
        }
        Call::Page(
            PageCall::Consent { town, .. }
            | PageCall::Act { town, .. }
            | PageCall::Withdraw { town, .. }
            | PageCall::Restore { town, .. },
        ) => (Action::Page, Some(town.as_ref())),
        _ => {
            return Err(Failure::new(
                "a device asks this of the machine it concerns itself, or not at all: only calls about a project are made for it",
            ));
        }
    };
    member.scope.allows(&here, action, None).map_err(refuse)?;
    match town {
        Some(Some(town)) if !crate::admission::in_scope(node, &member.scope, town) => {
            return Err(refuse(Denied::Project));
        }
        Some(None) => return Err(Failure::new(format!("{from} named no project; a device names the one it means"))),
        _ => {}
    }
    // What the call reaches on other machines, the device's scope must
    // reach too: this machine reaches them as itself.
    match &call {
        Call::Page(PageCall::Act { action, .. }) => {
            let target = match action {
                PageInvocation::Run { machine, .. } => machine,
                PageInvocation::Say { to, .. } => &to.machine,
                PageInvocation::Journal { .. } => &here,
            };
            member.scope.allows(target, Action::Page, None).map_err(refuse)?;
        }
        Call::Journal(JournalCall::Add { attach, .. }) => {
            for attached in attach {
                if attached.machine.as_ref().is_some_and(|machine| *machine != here) {
                    return Err(Failure::new(format!(
                        "{from} attached a file of another machine; a device attaches what it uploads to this one"
                    )));
                }
                member.scope.allows(&here, Action::Read, None).map_err(refuse)?;
            }
        }
        _ => {}
    }
    let user = Peer::Command { resident: None, session: None };
    let reply = match call {
        Call::Page(call) => crate::pages::call(node, &crate::town::caller::Caller::User, from, call).await?,
        call => match crate::server::handle(node, &user, call).await? {
            Reply::Towns { towns } => Reply::Towns {
                towns: towns
                    .into_iter()
                    .filter(|summary| crate::admission::in_scope(node, &member.scope, &summary.town))
                    .collect(),
            },
            reply => reply,
        },
    };
    let reply = serde_json::to_value(reply).map_err(|error| Failure::new(format!("writing the answer: {error}")))?;
    Ok(RemoteReply::Relayed { answer: reply })
}
