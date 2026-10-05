//! The local World contract carried either to a resident's machine or to
//! one online machine that asks the World on the device's behalf.

use fastctx_net::link::CallFailure;
use fastctx_types::remote::{RemoteCall, RemoteReply};
use fastctx_types::town::Looked;
use fastctx_types::{Address, Call, JournalCall, MachineName, ProjectCall, Reply, ResidentId};

use super::{Device, Session, failed, link_state};
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
        let (machine, remote, resident) = match call {
            Call::Talk { to, machine, text } => {
                let machine = session.locate(&to, machine).await?;
                (machine, RemoteCall::Talk { to: to.clone(), text }, Some(to))
            }
            Call::Open { agent, town, node, text, model, reasoning_effort } => {
                let machine =
                    node.ok_or_else(|| failed("choose a machine to open a conversation; no agents live on a device"))?;
                session.machine(&machine)?;
                (machine, RemoteCall::Open { agent, town, text, model, reasoning_effort }, None)
            }
            Call::Stop { resident, machine } => {
                let machine = session.locate(&resident, machine).await?;
                (machine, RemoteCall::Stop { resident: resident.clone() }, Some(resident))
            }
            Call::Control { resident, machine, control } => {
                let machine = session.locate(&resident, machine).await?;
                (machine, RemoteCall::Control { resident: resident.clone(), control }, Some(resident))
            }
            Call::Offers { resident, machine } => {
                let machine = session.locate(&resident, machine).await?;
                (machine, RemoteCall::Offers { resident: resident.clone() }, Some(resident))
            }
            call @ (Call::Who { .. }
            | Call::Since { .. }
            | Call::Look { .. }
            | Call::Agents { .. }
            | Call::Nodes
            | Call::Project(_)
            | Call::Journal(_)
            | Call::Invite { .. }
            | Call::Revoke { .. }) => {
                let (mut reply, machine) = session.relay(call).await?;
                if let Reply::Nodes(nodes) = &mut reply {
                    // The list comes from the relay; "here" and the link
                    // describe the device displaying it, not that relay.
                    nodes.machine = session.link.membership().name.clone();
                    nodes.world = Some(session.link.membership().world.to_string());
                    nodes.link = link_state(&session.link);
                }
                return Ok((reply, machine));
            }
            _ => return Err(failed("this call belongs to a local node, not a device")),
        };
        let answer = session.link.call(&machine, remote.clone()).await.map_err(failed)?.map_err(failed)?;
        let address = resident.map(|resident| Address { machine: machine.clone(), resident });
        let reply = match (remote, answer) {
            (RemoteCall::Talk { .. }, RemoteReply::Said { landed }) => {
                Reply::Spoken { to: address.expect("talk resident"), landed }
            }
            (RemoteCall::Open { .. }, RemoteReply::Opened { resident, landed }) => {
                Reply::Opened { resident: Address { machine: machine.clone(), resident }, landed }
            }
            (RemoteCall::Stop { .. }, RemoteReply::Stopped { stopped }) => {
                Reply::Stopped { resident: address.expect("stop resident"), stopped }
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
        let (reply, _) = self.relay(Call::Look { ids: vec![resident.to_string()] }).await?;
        let Reply::Looked { looked } = reply else {
            return Err(failed("the relay answered something other than a resident lookup"));
        };
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

    async fn relay(&self, call: Call) -> Result<(Reply, MachineName), Error> {
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
            Call::Who { .. }
                | Call::Since { .. }
                | Call::Look { .. }
                | Call::Agents { .. }
                | Call::Nodes
                | Call::Project(ProjectCall::List | ProjectCall::Show { .. } | ProjectCall::Map { .. })
                | Call::Journal(JournalCall::List { .. } | JournalCall::Read { .. })
        );
        let remote = RemoteCall::Relay { made: serde_json::to_value(&call).map_err(failed)? };
        let mut unanswered = Vec::new();
        for machine in machines {
            // A relay gets the existing three-heartbeat liveness window.
            // After that reads can move on; writes may already have acted,
            // so resubmitting them would risk duplicate entries/invitations.
            let answer =
                tokio::time::timeout(fastctx_net::wire::SILENCE, self.link.call(&machine, remote.clone())).await;
            match answer {
                Ok(Ok(Ok(RemoteReply::Relayed { answer }))) => {
                    return Ok((serde_json::from_value(answer).map_err(failed)?, machine));
                }
                Ok(Ok(Ok(_))) => return Err(failed(format!("{machine} answered something other than a relayed call"))),
                Ok(Ok(Err(error))) => return Err(failed(format!("on {machine}: {error}"))),
                Ok(Err(error @ CallFailure::NotConnected { .. })) => return Err(failed(error)),
                Ok(Err(error @ CallFailure::Unknown { .. })) if !repeatable => return Err(failed(error)),
                Ok(Err(error)) => unanswered.push(error.to_string()),
                Err(_) if !repeatable => {
                    return Err(failed(format!(
                        "{machine} did not answer within {} s; whether it acted is not known; this write was not sent again",
                        fastctx_net::wire::SILENCE.as_secs()
                    )));
                }
                Err(_) => unanswered
                    .push(format!("{machine} did not answer within {} s", fastctx_net::wire::SILENCE.as_secs())),
            }
        }
        Err(failed(format!("no online machine answered: {}", unanswered.join("; "))))
    }
}
