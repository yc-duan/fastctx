//! Which letters this machine takes (`world-network.md`, 管理权与范围). A
//! letter carries words or news to one of its residents, and is checked
//! here against the scope of the member that sent it, as a call is.
//!
//! Words from a resident of the sending machine are taken when the
//! sender's scope lets it talk to that resident. News that answers
//! something, a colleague's answer or a job that ended, is taken from the
//! machine it happened on, and only for a resident that reached out to
//! that machine itself: its actions name it. What speaks as the user, a
//! page or the World never comes in a letter: the user speaks from their
//! own devices, a page's words come from the machine that admitted its
//! action, and the World's are this machine's own.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use fastctx_net::scope::{Action, Denied};
use fastctx_net::trust::Member;
use fastctx_types::remote::Letter;
use fastctx_types::residents::Notice;
use fastctx_types::session::Speaker;
use fastctx_types::{MachineName, ResidentId};

use crate::Node;

/// Refuses a letter `member` may not send, with why.
pub async fn admit(node: &Arc<Node>, member: &Member, letter: &Letter) -> Result<(), String> {
    let from = &member.name;
    match letter {
        // Whose resident left is the sender's own; a fragment is checked
        // whole, once every part of it is here.
        Letter::Left { .. } | Letter::Fragment { .. } => Ok(()),
        Letter::Speak { to, .. } => may_talk(node, member, to),
        Letter::Notice { to, notice } => match notice {
            Notice::Words { from: Speaker::Resident { address }, .. } if address.machine == *from => {
                may_talk(node, member, to)
            }
            Notice::Words { from: Speaker::Resident { address }, .. } => {
                Err(format!("{from} sent words of {address}, a resident of another machine"))
            }
            Notice::Words { .. } | Notice::Attached { .. } => {
                Err(format!("{from} sent words as the user, a page or the World, which no letter carries"))
            }
            Notice::Answer { colleague, .. } | Notice::Health { colleague, .. } if colleague.machine == *from => {
                answers(node, member, to).await
            }
            Notice::JobEnded { machine, .. } if machine == from => answers(node, member, to).await,
            Notice::Answer { .. } | Notice::Health { .. } | Notice::JobEnded { .. } => {
                Err(format!("{from} told of something that happened on another machine"))
            }
        },
    }
}

/// Whether `member`'s scope lets it talk to the resident `to` here.
fn may_talk(node: &Node, member: &Member, to: &ResidentId) -> Result<(), String> {
    let here = node.machine();
    let refuse = |denied: Denied| format!("{here} refused a letter from {}: {denied}", member.name);
    member.scope.allows(&here, Action::Talk, Some(&to.agent())).map_err(refuse)?;
    if let Some(Some(town)) = node.residents.town_of(to)
        && !super::in_scope(node, &member.scope, &town)
    {
        return Err(refuse(Denied::Project));
    }
    Ok(())
}

/// Whether news from `member` may reach `to`: it may talk to it, or `to`
/// reached out to `member`'s machine itself: it is asking it something now,
/// or its actions, in any project it has lived in, name it.
async fn answers(node: &Arc<Node>, member: &Member, to: &ResidentId) -> Result<(), String> {
    let Err(refused) = may_talk(node, member, to) else { return Ok(()) };
    if node.residents.town_of(to).is_none() {
        return Err(format!("no resident {to} lives on {}", node.machine()));
    }
    if node.reaching.holds(to, &member.name) {
        return Ok(());
    }
    let (world, to_owned, machine) = (node.world.clone(), to.clone(), member.name.clone());
    let reached = tokio::task::spawn_blocking(move || {
        let towns = world.towns().unwrap_or_default();
        std::iter::once(None)
            .chain(towns.iter().map(Some))
            .any(|town| reached_out(&world.resident(town, &to_owned).actions(), &machine))
    })
    .await
    .unwrap_or(false);
    if reached { Ok(()) } else { Err(format!("{refused}, and {to} asked nothing of it")) }
}

/// Whether the actions recorded in `actions` name `machine`.
fn reached_out(actions: &std::path::Path, machine: &MachineName) -> bool {
    let Ok(text) = std::fs::read_to_string(actions) else { return false };
    text.lines()
        .filter_map(|line| serde_json::from_str::<fastctx_types::residents::Action>(line).ok())
        .any(|action| action.machine == *machine)
}

/// The machines residents here are asking something of right now: news
/// from one may arrive before the asking is recorded in their actions.
#[derive(Default)]
pub struct Reaching(Mutex<HashMap<(ResidentId, MachineName), usize>>);

impl Reaching {
    /// Notes that `resident` is asking `machine` something until the
    /// returned guard is dropped.
    pub fn hold(&self, resident: &ResidentId, machine: &MachineName) -> Held<'_> {
        let key = (resident.clone(), machine.clone());
        *self.0.lock().expect("never poisoned").entry(key.clone()).or_default() += 1;
        Held { reaching: self, key }
    }

    fn holds(&self, resident: &ResidentId, machine: &MachineName) -> bool {
        self.0.lock().expect("never poisoned").contains_key(&(resident.clone(), machine.clone()))
    }
}

pub struct Held<'a> {
    reaching: &'a Reaching,
    key: (ResidentId, MachineName),
}

impl Drop for Held<'_> {
    fn drop(&mut self) {
        let mut held = self.reaching.0.lock().expect("never poisoned");
        if let Some(count) = held.get_mut(&self.key) {
            *count -= 1;
            if *count == 0 {
                held.remove(&self.key);
            }
        }
    }
}
