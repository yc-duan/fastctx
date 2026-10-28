//! Which letters this machine takes (`world-network.md`, 四种话). A letter
//! carries words or news to one of its residents, and is checked here as a
//! call is: the purpose it was sent for must be the one its contents are,
//! and the resident it is for must work for the account it was sent for.
//!
//! Words come from a resident of the sending machine. News that answers
//! something, a colleague's answer or a job that ended, comes from the
//! machine it happened on. What speaks as the user, a page or the World
//! never comes in a letter: the user speaks from their own devices, a
//! page's words come from the machine that admitted its action, and the
//! World's are this machine's own.

use fastctx_net::link::Caller;
use fastctx_types::access::{HubId, Purpose};
use fastctx_types::remote::Letter;
use fastctx_types::residents::Notice;
use fastctx_types::session::Speaker;
use fastctx_types::{AccountName, MachineName, ResidentId};

use crate::Node;

/// Refuses a letter `caller` may not send, with why.
pub fn admit(
    node: &std::sync::Arc<Node>,
    hub: &HubId,
    caller: &Caller,
    purpose: Purpose,
    letter: &Letter,
) -> Result<(), String> {
    let from = &caller.machine;
    // A letter written before its sender was renamed names it as it was.
    let view = node.link_of(hub).map(|link| link.view());
    let sender = |machine: &MachineName| {
        machine == from || *machine == caller.sent_as || view.as_ref().is_some_and(|view| view.same(machine, from))
    };
    let mut also: &[AccountName] = &[];
    let (to, carries) = match letter {
        Letter::ControlResult { .. } => return expect(purpose, Purpose::Echo),
        Letter::Control { command } => {
            expect(purpose, Purpose::Hands)?;
            return crate::control_letters::admit(node, hub, caller, command).map_err(|error| error.message);
        }
        // A fragment is checked whole, once every part of it is here.
        Letter::Fragment { .. } => return Ok(()),
        // Whose resident left is the sender's own.
        Letter::Left { .. } => return expect(purpose, Purpose::Echo),
        Letter::JobFacts { to, update } if sender(&update.machine) => (to, Purpose::Echo),
        Letter::JobFacts { .. } => return Err(format!("{from} sent job facts of another machine")),
        Letter::Speak { to, accounts, .. } => {
            also = accounts;
            (to, Purpose::Words)
        }
        Letter::Notice { to, notice } => match notice.as_ref() {
            Notice::Words {
                from: Speaker::Resident { address },
                goal_report: Some(report),
                goal_question: None,
                ..
            } if *address == report.colleague
                && sender(&address.machine)
                && report.account.hub == *hub
                && report.account.account == caller.account =>
            {
                (to, Purpose::Echo)
            }
            Notice::Words { goal_report: Some(_), .. } => {
                return Err(format!("{from} sent a goal report of another machine or account"));
            }
            Notice::Words { from: Speaker::Resident { address }, goal_question: Some(question), .. }
                if *address == question.colleague
                    && sender(&address.machine)
                    && question
                        .account
                        .as_ref()
                        .is_none_or(|account| account.hub == *hub && account.account == caller.account) =>
            {
                (to, Purpose::Echo)
            }
            Notice::Words { goal_question: Some(_), .. } => {
                return Err(format!("{from} sent a question of another machine or account"));
            }
            Notice::Words { from: Speaker::Resident { address }, .. } if sender(&address.machine) => {
                (to, Purpose::Words)
            }
            Notice::Words { from: Speaker::Resident { address }, .. } => {
                return Err(format!("{from} sent words of {address}, a resident of another machine"));
            }
            Notice::Words { .. } | Notice::Attached { .. } | Notice::World { .. } => {
                return Err(format!("{from} sent words as the user, a page or the World, which no letter carries"));
            }
            Notice::Answer { colleague, .. }
            | Notice::Health { colleague, .. }
            | Notice::Question { colleague, .. }
                if sender(&colleague.machine) =>
            {
                (to, Purpose::Echo)
            }

            Notice::JobEnded { machine, .. } if sender(machine) => (to, Purpose::Echo),
            Notice::Sentinel { observation } if sender(&observation.machine) => (to, Purpose::Echo),
            Notice::Answer { .. }
            | Notice::Health { .. }
            | Notice::Question { .. }
            | Notice::JobEnded { .. }
            | Notice::Sentinel { .. } => {
                return Err(format!("{from} told of something that happened on another machine"));
            }
        },
    };
    if carries == Purpose::Words && (node.residents.is_deleting(to) || node.residents.removed(to).is_some()) {
        return Err(format!("{to} is being deleted or was removed"));
    }
    expect(purpose, carries)?;
    serves(node, hub, caller, also, to)
}

fn expect(declared: Purpose, carries: Purpose) -> Result<(), String> {
    if declared == carries { Ok(()) } else { Err(format!("it was sent as {declared:?} and carries {carries:?}")) }
}

/// Whether `to` lives here and works for the account the letter was sent
/// for, or for one of the others the speaker works for: each of those this
/// machine checks itself, as the hub checked the one it was sent for.
fn serves(node: &Node, hub: &HubId, caller: &Caller, also: &[AccountName], to: &ResidentId) -> Result<(), String> {
    let Some((about, _)) = node.residents.about(to) else {
        return Err(format!("no resident {to} lives on {}", node.machine()));
    };
    if node.serves(&about.acting, hub, &caller.account) {
        return Ok(());
    }
    let here = node.machine();
    let view = node.link_of(hub).map(|link| link.view());
    let speaks_for = |account: &&AccountName| {
        view.as_ref().is_some_and(|view| view.admits(&caller.machine, account, &here, Purpose::Words).is_ok())
    };
    if also.iter().filter(speaks_for).any(|account| node.serves(&about.acting, hub, account)) {
        return Ok(());
    }
    Err(format!("{to} on {here} works for none of the accounts the words came for"))
}
