//! What is seen of this machine's answers to the World-wide asks
//! (`world-gui.md`, 侧栏、首页与机器页上列的对话; `world-network.md`, 近况):
//! a conversation, and what it did, are seen by the accounts it works for
//! alone, whoever else may work on this machine, its owner and the
//! network's administrator among them. What runs here now is the machine's
//! (`world-network.md`, 共享机器与智能体协助): an account that may work here
//! sees every job running, each with the account it works for; the jobs
//! that ended are history, seen as their conversations are. Which agents are
//! logged in here is the machine's too: an account that may work here sees
//! them all, one some of their logins assist sees those alone.
//!
//! Another machine's account asks through the door (`Asker`); this
//! machine's own caller asks as whoever it works for (`Acting`), its own
//! user seeing the conversations opened here and those working for an
//! account this computer is logged in to.

use fastctx_types::access::{Acting, HubAccount};
use fastctx_types::agents::AgentsReply;
use fastctx_types::run::JobOwner;
use fastctx_types::session::control::Offers;
use fastctx_types::since::{SinceKind, SinceReply, SinceTown};
use fastctx_types::town::{Looked, WhoReply};
use fastctx_types::{Address, ResidentId, TownName};

use super::Asker;
use crate::Node;

/// Who looks at this machine's records.
#[derive(Clone, Copy)]
pub enum Looker<'a> {
    /// An account of another machine, through the door.
    Asker(&'a Asker),
    /// This machine's own caller, as whoever it works for.
    Here(&'a Acting),
}

impl Looker<'_> {
    /// Whether what works for `account` is the looker's to see.
    pub(crate) fn sees_account(self, node: &Node, account: &HubAccount) -> bool {
        match self {
            Looker::Asker(asker) => account.hub == asker.hub && account.account == asker.account,
            Looker::Here(acting) => node.serves(acting, &account.hub, &account.account),
        }
    }

    /// Whether a conversation of this machine working for `acting` is the
    /// looker's to see: the two work for an account in common, a
    /// conversation opened here working for the accounts this computer is
    /// logged in to.
    pub(crate) fn sees(self, node: &Node, acting: &Acting) -> bool {
        match (self, acting) {
            (Looker::Asker(asker), _) => node.serves(acting, &asker.hub, &asker.account),
            (Looker::Here(_), Acting::Accounts { accounts }) => {
                accounts.iter().any(|account| self.sees_account(node, account))
            }
            (Looker::Here(Acting::Here), Acting::Here) => true,
            (Looker::Here(Acting::Accounts { accounts }), Acting::Here) => {
                accounts.iter().any(|account| node.serves(&Acting::Here, &account.hub, &account.account))
            }
        }
    }

    /// Whether the looker may use the project `town`, as the views of the
    /// hubs say: a project's records go to its accounts alone. One kept on
    /// no hub is this computer's own, its user's.
    fn in_project(self, node: &Node, town: &TownName) -> bool {
        let mut kept = false;
        for link in node.links() {
            let view = link.view();
            let Some(project) = view.projects.iter().find(|project| project.name == *town) else { continue };
            kept = true;
            let may = match self {
                Looker::Asker(asker) => {
                    link.membership().id == asker.hub && view.in_project(&asker.account, &project.town)
                }
                Looker::Here(acting) => {
                    node.accounts_on(&link, acting).iter().any(|account| view.in_project(account, &project.town))
                }
            };
            if may {
                return true;
            }
        }
        !kept && matches!(self, Looker::Here(Acting::Here))
    }
}

/// Whether `looker` sees the resident `id` of this machine.
pub fn resident(node: &Node, looker: Looker, id: &ResidentId) -> bool {
    node.residents.about(id).is_some_and(|(about, _)| looker.sees(node, &about.acting))
}

/// Whether `looker` sees what the resident at `address` did here. This
/// machine's records keep what its own residents did; a resident of another
/// machine is seen through the jobs it ran here (`owner`).
fn address(node: &Node, looker: Looker, address: &Address) -> bool {
    node.is_me(&address.machine) && resident(node, looker, &address.resident)
}

/// Whether `looker` sees a job: one a resident of this machine started, as
/// that resident; one run here for a resident of another machine, by the
/// account it runs for; a page's, by whoever may work here, the call's
/// project check covering the project.
fn owner(node: &Node, looker: Looker, owner: &JobOwner, account: Option<&HubAccount>) -> bool {
    match owner {
        JobOwner::Resident(owner) if node.is_me(&owner.machine) => resident(node, looker, &owner.resident),
        JobOwner::Resident(_) => account.is_some_and(|account| looker.sees_account(node, account)),
        JobOwner::Page { .. } => match looker {
            Looker::Asker(asker) => asker.hands,
            Looker::Here(acting) => node.works_here(acting).is_ok(),
        },
    }
}

pub fn who(node: &Node, asker: &Asker, mut reply: WhoReply) -> WhoReply {
    let looker = Looker::Asker(asker);
    unseen_by_account(node, looker, Some(&asker.hub), &mut reply);
    reply.residents.retain(|now| resident(node, looker, &now.about.id));
    reply.jobs.retain(|job| running(node, asker, &job.owner, job.account.as_ref()));
    reply.history = reply.history.map(|history| since(node, looker, history));
    reply
}

/// This machine's own answer to whoever asks here and may work here, as
/// `acting`: the conversations working for its accounts, and every job
/// running, each with its account. Another account's conversation is not
/// sent even to this computer's own interface: it is not left to the
/// interface to hide.
pub fn who_here(node: &Node, acting: &Acting, mut reply: WhoReply) -> WhoReply {
    let looker = Looker::Here(acting);
    reply.history = reply.history.map(|history| since(node, looker, history));
    unseen_by_account(node, looker, None, &mut reply);
    reply.residents.retain(|now| resident(node, looker, &now.about.id));
    if node.works_here(acting).is_err() {
        reply.jobs.retain(|job| owner(node, looker, &job.owner, job.account.as_ref()));
    }
    reply
}

/// What this machine's own caller, who may work here, finds looking up an
/// id here: a conversation working for its accounts; a job running here,
/// whosever it is.
pub fn looked_here(node: &Node, acting: &Acting, mut looked: Option<Looked>) -> Option<Looked> {
    goal_reports(node, Looker::Here(acting), &mut looked);
    match &looked {
        Some(Looked::Resident(detail)) => {
            let at =
                Address { machine: detail.resident.about.machine.clone(), resident: detail.resident.about.id.clone() };
            if address(node, Looker::Here(acting), &at) { looked } else { None }
        }
        _ => looked,
    }
}

/// Gives each job of a conversation `looker` does not see the account it
/// works for, on `hub` where one is named: whose the job is stays said,
/// the conversation itself unseen.
fn unseen_by_account(node: &Node, looker: Looker, hub: Option<&fastctx_types::access::HubId>, reply: &mut WhoReply) {
    for job in &mut reply.jobs {
        if job.account.is_none()
            && let Some(owner) = job.owner.resident()
            && node.is_me(&owner.machine)
            && !resident(node, looker, &owner.resident)
        {
            job.account = account_of(node, hub, &owner.resident);
        }
    }
}

/// Whether `asker` sees a job running here, or one looked up by its id:
/// whoever may work here sees them all.
fn running(node: &Node, asker: &Asker, job_owner: &JobOwner, account: Option<&HubAccount>) -> bool {
    asker.hands || owner(node, Looker::Asker(asker), job_owner, account)
}

/// The account the resident `id` of this machine works for, on `hub` where
/// it works for one there: the first it names, or for a conversation
/// opened here, the first this machine is logged in to.
fn account_of(node: &Node, hub: Option<&fastctx_types::access::HubId>, id: &ResidentId) -> Option<HubAccount> {
    let (about, _) = node.residents.about(id)?;
    match about.acting {
        Acting::Accounts { accounts } => {
            accounts.iter().find(|account| Some(&account.hub) == hub).or(accounts.first()).cloned()
        }
        Acting::Here => {
            let link = match hub {
                Some(hub) => node.link_of(hub)?,
                None => node.links().into_iter().next()?,
            };
            let account = link.view().me()?.logins.iter().next().cloned()?;
            Some(HubAccount { hub: link.membership().id.clone(), account })
        }
    }
}

pub fn since(node: &Node, looker: Looker, mut reply: SinceReply) -> SinceReply {
    let seen = |resident: &Address| address(node, looker, resident);
    // The other end of an invitation or an answer may live on another
    // machine, which answers for it; this one judges the end it keeps.
    let other_end = |resident: &Address| !node.is_me(&resident.machine) || seen(resident);
    let project = |town: &SinceTown| match town {
        SinceTown::Known { town: Some(town) } => looker.in_project(node, town),
        _ => false,
    };
    reply.events.retain(|event| match &event.what {
        SinceKind::FileAction { resident, .. }
        | SinceKind::ResidentBorn { resident, .. }
        | SinceKind::TurnEnded { resident, .. }
        | SinceKind::ResidentMoved { resident, .. }
        | SinceKind::ClaudeRegistered { resident, .. }
        | SinceKind::SubmissionUncertain { resident, .. } => seen(resident),
        SinceKind::JobEnded { owner: job_owner, account, .. } => owner(node, looker, job_owner, account.as_ref()),
        SinceKind::ColleagueAsked { asker: who, colleague, .. } => seen(who) && other_end(colleague),
        SinceKind::AnswerDelivery { colleague, to, .. } => seen(colleague) && other_end(to),
        SinceKind::JournalEntry { .. } | SinceKind::JournalDescription { .. } | SinceKind::ProjectVersion { .. } => {
            project(&event.town)
        }
        SinceKind::MachineConnected | SinceKind::MachineOffline => true,
    });
    reply.undated.retain(|fact| seen(&fact.resident));
    reply
}

/// Which agents are logged in here is the machine's: an account that may
/// work here sees them all, one some of their logins assist sees those alone,
/// without their account's windows.
pub fn agents(view: &fastctx_net::directory::View, asker: &Asker, mut reply: AgentsReply) -> AgentsReply {
    if asker.hands {
        return reply;
    }
    let here = view.me.clone();
    reply.agents.retain(|facts| {
        view.assisting(&asker.account, &here).any(|assist| {
            assist.agent == facts.agent
                && match (&assist.home, &facts.home) {
                    (None, None) => true,
                    (None, Some(home)) => home.found == fastctx_types::agents::HomeFound::Default,
                    (Some(dir), Some(home)) => crate::homes::same_dir(dir, &home.dir),
                    (Some(_), None) => false,
                }
        })
    });
    for facts in &mut reply.agents {
        facts.limits = fastctx_types::agents::Fact::unavailable(crate::logins::remote::OWNERS_WINDOWS);
    }
    reply
}

/// What a conversation offers, as the account it works for sees it: one
/// the login only assists is not told the login's account's windows.
pub fn offers(asker: &Asker, mut offers: Offers) -> Offers {
    if !asker.hands
        && let Some(usage) = offers.usage.as_mut()
    {
        usage.limits.clear();
        usage.limits_at = None;
        if let Some(raw) = usage.raw.as_object_mut() {
            raw.remove("rateLimits");
        }
    }
    offers
}

pub fn looked(node: &Node, asker: &Asker, mut looked: Option<Looked>) -> Option<Looked> {
    let looker = Looker::Asker(asker);
    goal_reports(node, looker, &mut looked);
    let seen = match &looked {
        None => true,
        Some(Looked::Resident(detail)) => address(
            node,
            looker,
            &Address { machine: detail.resident.about.machine.clone(), resident: detail.resident.about.id.clone() },
        ),
        Some(Looked::Job { record, .. }) => {
            let account = record.account.as_ref().or(record.on.as_ref().and_then(|on| on.account.as_ref()));
            running(node, asker, &record.owner, account)
        }
    };
    if seen { looked } else { None }
}

fn goal_reports(node: &Node, looker: Looker<'_>, looked: &mut Option<Looked>) {
    if let Some(Looked::Resident(detail)) = looked {
        detail.goal_reports.retain(|report| looker.sees_account(node, &report.account));
    }
}

pub fn job(node: &Node, asker: &Asker, job: &fastctx_types::town::RunningJob) -> bool {
    running(node, asker, &job.owner, job.account.as_ref())
}
