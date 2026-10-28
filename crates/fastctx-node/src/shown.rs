//! The World as an interface shows it (`fastctx_types::world::WorldNow`),
//! read from what this computer holds, asking no machine: its own residents
//! as they are, the other machines' as their hubs last told what is afoot
//! on them, the machines hub by hub as the views have them, with what each
//! said of itself when it last connected, the projects kept here, and the
//! accounts (`world-network.md`, 近况). Of the residents and jobs, only
//! those that work for an account logged in here are in it, wherever they
//! run (`world-gui.md`, 界面的结构).

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use fastctx_net::link::{Elsewhere, Status};
use fastctx_types::access::{Acting, HubId, Purpose};
use fastctx_types::ipc::Failure;
use fastctx_types::town::{MachineWho, WhoReply, WhoScope, WorldWho};
use fastctx_types::world::{Node as NodeView, NodesReply, WorldNow};
use fastctx_types::{MachineName, ResidentId};

use crate::Node;

pub(crate) async fn now(node: &Arc<Node>) -> Result<WorldNow, Failure> {
    let here = node.machine();
    let towns = crate::ledger::query::held(node).await?;
    let mut machines = vec![MachineWho::Answered { observation: None, reply: this(node).await? }];
    machines.extend(elsewhere(node));
    let who = WorldWho {
        machine: here.clone(),
        scope: WhoScope::World { towns: towns.clone() },
        you: None,
        machines,
        history: None,
        others: Vec::new(),
        involved: Vec::new(),
    };
    Ok(WorldNow { here, who, nodes: nodes(node), towns, accounts: crate::world::accounts(node) })
}

/// Whether what the hubs told of the other machines is as each session on
/// now first gave it: until then the World is not told, so that it does not
/// show the other machines' residents gone for a moment.
pub(crate) fn ready(node: &Node) -> bool {
    node.links().iter().all(|link| link.afoot_ready())
}

/// This computer's own residents and jobs that work for an account logged
/// in here, and how many agents are at work or waiting here, whosever they
/// are, save those of its logins assisting other accounts.
async fn this(node: &Arc<Node>) -> Result<WhoReply, Failure> {
    let mut reply = crate::server::who_here(node, true, None, None).await?;
    let mine = |acting: &Acting| match acting {
        Acting::Here => true,
        Acting::Accounts { accounts } => {
            accounts.iter().any(|account| node.serves(&Acting::Here, &account.hub, &account.account))
        }
    };
    let works_here = |acting: &Acting| match acting {
        Acting::Here => true,
        Acting::Accounts { accounts } => accounts.iter().any(|account| {
            node.link_of(&account.hub).is_some_and(|link| {
                let view = link.view();
                view.rights(&account.account, &view.me).hands
            })
        }),
    };
    let busy =
        reply.residents.iter().filter(|resident| resident.in_progress() && works_here(&resident.about.acting)).count();
    let kept: HashMap<ResidentId, bool> =
        reply.residents.iter().map(|resident| (resident.about.id.clone(), mine(&resident.about.acting))).collect();
    reply.residents.retain(|resident| mine(&resident.about.acting));
    reply.jobs.retain(|job| match (&job.account, job.owner.resident()) {
        (Some(account), _) => node.serves(&Acting::Here, &account.hub, &account.account),
        (None, Some(owner)) => node.is_me(&owner.machine) && kept.get(&owner.resident).copied().unwrap_or(false),
        // A page open here started it.
        (None, None) => true,
    });
    reply.busy = Some(u32::try_from(busy).unwrap_or(u32::MAX));
    Ok(reply)
}

/// The other machines the accounts here may look at, each as its hub last
/// told what is afoot on it.
fn elsewhere(node: &Node) -> Vec<MachineWho> {
    let mut told: HashMap<HubId, BTreeMap<MachineName, Elsewhere>> = HashMap::new();
    let mut machines = Vec::new();
    let links = node.links();
    for reachable in crate::around::reachable(node, &Acting::Here, Purpose::Look) {
        let view = reachable.link.view();
        let Some(member) = view.member(&reachable.name) else { continue };
        // Each hub tells of what there works for its own accounts logged in
        // here: the same machine on two hubs has what both tell.
        let mut afoot: Option<Elsewhere> = None;
        for link in &links {
            let Some(there) =
                link.view().machines().find(|machine| machine.key == reachable.key).map(|m| m.name.clone())
            else {
                continue;
            };
            let Some(told) = told.entry(link.membership().id.clone()).or_insert_with(|| link.afoot()).get(&there)
            else {
                continue;
            };
            let merged = afoot.get_or_insert_with(Elsewhere::default);
            merged.seen.residents.extend(told.seen.residents.iter().map(|(id, now)| (id.clone(), now.clone())));
            merged.seen.jobs.extend(told.seen.jobs.iter().map(|(id, job)| (id.clone(), job.clone())));
            merged.busy = merged.busy.max(told.busy);
        }
        let afoot = afoot.as_ref();
        let reply = |afoot: &Elsewhere| WhoReply {
            machine: reachable.name.clone(),
            residents: afoot.seen.residents.values().cloned().collect(),
            jobs: afoot.seen.jobs.values().cloned().collect(),
            history: None,
            busy: afoot.busy,
        };
        let name = reachable.name.clone();
        let current = matches!(reachable.link.status(), Status::Online { .. });
        machines.push(match (member.online, afoot) {
            (false, afoot) => MachineWho::Offline {
                observation: None,
                machine: name,
                since: member.since,
                last: afoot.map(|afoot| Box::new(reply(afoot))),
            },
            (true, Some(afoot)) => MachineWho::Answered { observation: None, reply: reply(afoot) },
            (true, None) if !current => MachineWho::NoAnswer {
                observation: None,
                machine: name,
                reason: "this computer is not connected to the hub now".to_string(),
            },
            (true, None) if !view.afoot => MachineWho::NoAnswer {
                observation: None,
                machine: name,
                reason: "the hub runs an older Fastctx, which does not pass on what is on each machine".to_string(),
            },
            (true, None) => MachineWho::NoAnswer {
                observation: None,
                machine: name,
                reason: "it runs an older Fastctx, which does not tell what is on it".to_string(),
            },
        });
    }
    machines.sort_by(|a, b| machine_of(a).cmp(machine_of(b)));
    machines
}

fn machine_of(who: &MachineWho) -> &MachineName {
    match who {
        MachineWho::Answered { reply, .. } => &reply.machine,
        MachineWho::Offline { machine, .. } | MachineWho::NoAnswer { machine, .. } => machine,
    }
}

/// This computer and, hub by hub, the machines its logins may work on, as
/// the views have them; what each machine is like as it said when it last
/// connected (`crate::facts`).
fn nodes(node: &Arc<Node>) -> NodesReply {
    let mut hubs = Vec::new();
    for link in node.links() {
        let view = link.view();
        // A view kept while the line to the hub is down still has the
        // machines, and says who was online then, not now.
        let current = matches!(link.status(), Status::Online { .. });
        let listed = link.last_view().is_some();
        let logins: Vec<fastctx_types::AccountName> =
            view.me().map(|me| me.logins.iter().cloned().collect()).unwrap_or_default();
        let mut machines = Vec::new();
        for machine in view.machines().filter(|machine| listed && !view.is_me(&machine.name)) {
            let rights: Vec<_> = logins
                .iter()
                .map(|account| (account.clone(), view.call_rights(&view.me, account, &machine.name)))
                .collect();
            let Some(account) = rights.iter().find(|(_, rights)| rights.hands).map(|(account, _)| account.clone())
            else {
                continue;
            };
            let facts = crate::facts::of(node, &link, machine, current.then_some(account));
            machines.push(NodeView {
                name: machine.name.clone(),
                written: node.written(&link, &machine.name).to_string(),
                online: current.then_some(machine.online),
                since: machine.since,
                through: machine.through.clone(),
                allows_direct: !machine.direct.is_empty(),
                path: Some(link.path_to(&machine.name)),
                facts,
                owner: machine.home.clone(),
                group: machine.group.clone(),
                rights,
            });
        }
        // This computer is decided as any machine is: a locked account
        // logged in here may not work on it either. The user at this
        // computer's own screen is another matter, and is not this.
        let here = logins.iter().map(|account| (account.clone(), view.call_rights(&view.me, account, &view.me))).collect();
        hubs.push(fastctx_types::world::HubNodes {
            hub: link.membership().id.clone(),
            name: view.hub_name.clone(),
            link: link.state(),
            here,
            logins,
            home: view.me().and_then(|me| me.home.clone()),
            machines,
        });
    }
    NodesReply { machine: node.machine(), facts: Some(crate::world::facts(node)), hubs }
}
