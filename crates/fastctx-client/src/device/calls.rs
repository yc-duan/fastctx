//! The local World contract carried to a resident's machine, asked of each
//! machine (`around`), or, for the projects' ledgers, to one machine that
//! answers from its own copy.

use fastctx_net::link::CallFailure;
use fastctx_types::pages::PageCall;
use fastctx_types::remote::{RemoteCall, RemoteReply};
use fastctx_types::town::Looked;
use fastctx_types::{Address, Call, JournalCall, MachineName, ProjectCall, Reply, ResidentId};

use super::{Device, Session, failed};
use crate::Error;

impl Device {
    pub(crate) async fn call(self: &std::sync::Arc<Self>, call: Call) -> Result<(Reply, MachineName), Error> {
        if let Call::Join { code, name } = call {
            let (world, name) = self.join(code, name).await?;
            return Ok((Reply::Joined { world, name: name.clone() }, name));
        }
        let session = self.connected()?;
        if matches!(call, Call::Leave) {
            let name = session.link.membership().name.clone();
            self.leave().await?;
            return Ok((Reply::Done, name));
        }
        session.check_identity().await?;
        let here = session.link.membership().name.clone();
        let (machine, remote, resident) = match call {
            Call::FindFiles { machine, request } => {
                let machine =
                    machine.ok_or_else(|| failed("choose a machine for file search; a device has no local files"))?;
                session.machine(&machine)?;
                (machine, RemoteCall::FindFiles { request }, None)
            }
            Call::ClaudeConsent { machine, accept } => {
                session.machine(&machine)?;
                (machine, RemoteCall::ClaudeConsent { accept }, None)
            }
            Call::Talk { to, machine, text, attachments } => {
                let machine = session.locate(&to, machine).await?;
                (machine, RemoteCall::Talk { to: to.clone(), text, attachments }, Some(to))
            }
            Call::Open { agent, town, node, text, model, reasoning_effort, attachments } => {
                let machine =
                    node.ok_or_else(|| failed("choose a machine to open a conversation; no agents live on a device"))?;
                session.machine(&machine)?;
                (machine, RemoteCall::Open { agent, town, text, model, reasoning_effort, attachments }, None)
            }
            Call::Stop { resident, machine } => {
                let machine = session.locate(&resident, machine).await?;
                (machine, RemoteCall::Stop { resident: resident.clone() }, Some(resident))
            }
            Call::Manage { resident, machine, action } => {
                let machine = session.locate(&resident, machine).await?;
                (machine, RemoteCall::Manage { resident: resident.clone(), action }, Some(resident))
            }
            Call::Control { resident, machine, control } => {
                let machine = session.locate(&resident, machine).await?;
                (machine, RemoteCall::Control { resident: resident.clone(), control }, Some(resident))
            }
            Call::Offers { resident, machine } => {
                let machine = session.locate(&resident, machine).await?;
                (machine, RemoteCall::Offers { resident: resident.clone() }, Some(resident))
            }
            // What asks of the whole World, the device asks of each machine
            // itself (`around`).
            Call::Who { all, town } => return Ok((Reply::Who(session.who(all, town).await?), here)),
            Call::Since { since, town } => return Ok((Reply::Since(session.since(since, town).await?), here)),
            Call::Look { ids } => return Ok((Reply::Looked { looked: session.look(ids).await? }, here)),
            Call::Agents { machine } => return Ok((Reply::Agents(session.agents(machine).await?), here)),
            Call::Nodes => return Ok((Reply::Nodes(session.nodes().await?), here)),
            // What only the projects' ledgers hold, one machine answers from
            // its own copy, for what this device's scope covers.
            call @ (Call::Project(_)
            | Call::Journal(_)
            | Call::Page(
                PageCall::Read { .. }
                | PageCall::List { .. }
                | PageCall::History { .. }
                | PageCall::Consent { .. }
                | PageCall::Act { .. }
                | PageCall::Withdraw { .. }
                | PageCall::Restore { .. },
            )) => return session.relay(call).await,
            _ => return Err(failed("this call belongs to a local node, not a device")),
        };
        let answer = session.link.call(&machine, remote.clone()).await.map_err(failed)?.map_err(failed)?;
        let address = resident.map(|resident| Address { machine: machine.clone(), resident });
        let reply = match (remote, answer) {
            (RemoteCall::FindFiles { .. }, RemoteReply::FilesFound(reply)) => Reply::FilesFound(reply),
            (RemoteCall::ClaudeConsent { .. }, RemoteReply::ClaudeConsent { accepted, changed }) => {
                Reply::ClaudeConsent { machine: machine.clone(), accepted, changed }
            }
            (RemoteCall::Talk { .. }, RemoteReply::Said { landed }) => {
                Reply::Spoken { to: address.expect("talk resident"), landed }
            }
            (RemoteCall::Open { .. }, RemoteReply::Opened { resident, landed }) => {
                Reply::Opened { resident: Address { machine: machine.clone(), resident }, landed }
            }
            (RemoteCall::Stop { .. }, RemoteReply::Stopped { stopped }) => {
                Reply::Stopped { resident: address.expect("stop resident"), stopped }
            }
            (RemoteCall::Manage { .. }, RemoteReply::Managed(managed)) => {
                Reply::Managed { resident: address.expect("manage resident"), managed }
            }
            (RemoteCall::Control { .. }, RemoteReply::Controlled(controlled)) => {
                Reply::Controlled { resident: address.expect("control resident"), controlled }
            }
            (RemoteCall::Offers { .. }, RemoteReply::Offers(offers)) => {
                Reply::Offers { resident: address.expect("offers resident"), offers }
            }
            _ => {
                return Err(failed(format!(
                    "{machine} answered something other than the requested conversation operation"
                )));
            }
        };
        Ok((reply, machine))
    }
}

impl Session {
    pub(super) fn machine(&self, machine: &MachineName) -> Result<(), Error> {
        match self.link.members().0.get(machine) {
            Some(member) if member.kind.is_machine() => Ok(()),
            Some(_) => Err(failed(format!("{machine} is a device; no agents live on it"))),
            None => Err(failed(format!("{machine} is not a trusted machine of this World"))),
        }
    }

    pub(super) async fn locate(
        &self,
        resident: &ResidentId,
        machine: Option<MachineName>,
    ) -> Result<MachineName, Error> {
        if let Some(machine) = machine {
            self.machine(&machine)?;
            return Ok(machine);
        }
        let looked = self.look(vec![resident.to_string()]).await?;
        let mut machines = std::collections::BTreeSet::new();
        let mut unasked = Vec::new();
        for item in looked {
            unasked.extend(item.unasked);
            for found in item.found {
                if let Looked::Resident(detail) = found
                    && detail.resident.about.id == *resident
                {
                    machines.insert(detail.resident.about.machine.clone());
                }
            }
        }
        if machines.len() == 1 {
            let machine = machines.pop_first().expect("one machine");
            self.machine(&machine)?;
            return Ok(machine);
        }
        if machines.is_empty() {
            return Err(failed(format!(
                "no answering machine has resident {resident}; machines not asked: {}",
                unasked.join("; ")
            )));
        }
        Err(failed(format!(
            "resident {resident} lives on more than one machine ({}); choose one",
            machines.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ")
        )))
    }

    pub(super) async fn relay(&self, call: Call) -> Result<(Reply, MachineName), Error> {
        let (table, presence) = self.link.members();
        let machines: Vec<_> = table
            .machines()
            .filter(|member| presence.iter().any(|present| present.name == member.name && present.online))
            .map(|member| member.name.clone())
            .collect();
        if machines.is_empty() {
            return Err(failed("no machines of this World are online; the device cannot ask a node to act for it"));
        }
        let repeatable = matches!(
            call,
            Call::Project(ProjectCall::List | ProjectCall::Show { .. } | ProjectCall::Map { .. })
                | Call::Journal(JournalCall::List { .. } | JournalCall::Read { .. })
                | Call::Page(PageCall::Read { .. } | PageCall::List { .. } | PageCall::History { .. })
        );
        let remote = RemoteCall::Relay { made: serde_json::to_value(&call).map_err(failed)? };
        let mut unanswered = Vec::new();
        for machine in machines {
            // Heartbeats establish connection liveness, not a deadline for
            // completing a query or transferring its answer. Only a reported
            // delivery/connection failure permits trying another machine.
            match self.link.call(&machine, remote.clone()).await {
                Ok(Ok(RemoteReply::Relayed { answer })) => {
                    return Ok((serde_json::from_value(answer).map_err(failed)?, machine));
                }
                Ok(Ok(_)) => return Err(failed(format!("{machine} answered something other than a relayed call"))),
                Ok(Err(error)) => return Err(failed(format!("on {machine}: {error}"))),
                Err(error @ CallFailure::NotConnected { .. }) => return Err(failed(error)),
                Err(error @ CallFailure::Unknown { .. }) if !repeatable => {
                    return Err(failed(format!("{error}; this write was not sent again")));
                }
                Err(error) => unanswered.push(error.to_string()),
            }
        }
        Err(failed(format!("no online machine answered: {}", unanswered.join("; "))))
    }
}
