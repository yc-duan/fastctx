//! Account-filtered agent inventories use the same machine grouping.

use fastctx_types::agents::{AgentsReply, MachineAgents};
use fastctx_types::logins::{AgentLogins, MachineLogins, MachineLoginsHeard};
use fastctx_types::observation::union;
use fastctx_types::remote::RemoteReply;

use super::Answers;

impl Answers {
    pub fn agents(mut self) -> MachineAgents {
        let mut merged: Option<AgentsReply> = None;
        for (account, reply) in std::mem::take(&mut self.replies) {
            match reply {
                RemoteReply::Agents(mut reply) => {
                    reply.machine = self.source.written.name.clone();
                    match &mut merged {
                        None => merged = Some(reply),
                        Some(merged) => {
                            let newer = reply.checked_at >= merged.checked_at;
                            for agent in reply.agents {
                                if let Some(held) = merged
                                    .agents
                                    .iter_mut()
                                    .find(|held| held.agent == agent.agent && held.home == agent.home)
                                {
                                    if newer {
                                        *held = agent;
                                    }
                                } else {
                                    merged.agents.push(agent);
                                }
                            }
                            merged.checked_at = merged.checked_at.max(reply.checked_at);
                            merged.elapsed_ms = merged.elapsed_ms.max(reply.elapsed_ms);
                        }
                    }
                }
                _ => self.unexpected(account),
            }
        }
        let observation = self.observation();
        match merged {
            Some(reply) => MachineAgents::Answered { reply, observation },
            None => match self.offline() {
                Some(since) => MachineAgents::Offline { machine: self.source.written.name.clone(), since, observation },
                None => MachineAgents::NoAnswer {
                    machine: self.source.written.name.clone(),
                    reason: self.missing().join("; "),
                    observation,
                },
            },
        }
    }

    pub fn logins(mut self) -> MachineLoginsHeard {
        let mut merged: Option<Box<MachineLogins>> = None;
        for (account, reply) in std::mem::take(&mut self.replies) {
            match reply {
                RemoteReply::AgentLogins(mut reply) => {
                    reply.machine = self.source.written.name.clone();
                    match &mut merged {
                        None => merged = Some(reply),
                        Some(merged) => {
                            merge_logins(&mut merged.codex, reply.codex);
                            merge_logins(&mut merged.claude, reply.claude);
                            merged.scan = reply.scan.or(merged.scan.take());
                            union(&mut merged.folders, reply.folders);
                        }
                    }
                }
                _ => self.unexpected(account),
            }
        }
        let observation = self.observation();
        match merged {
            Some(logins) => MachineLoginsHeard::Answered { logins, observation },
            None => match self.offline() {
                Some(since) => {
                    MachineLoginsHeard::Offline { machine: self.source.written.name.clone(), since, observation }
                }
                None => MachineLoginsHeard::NoAnswer {
                    machine: self.source.written.name.clone(),
                    reason: self.missing().join("; "),
                    observation,
                },
            },
        }
    }
}

fn merge_logins(into: &mut AgentLogins, mut more: AgentLogins) {
    // Program references are indexes into each answer's inventory.
    let indices: Vec<_> = more
        .programs
        .into_iter()
        .map(|program| {
            if let Some(at) = into.programs.iter().position(|held| held == &program) {
                at as u32
            } else {
                into.programs.push(program);
                (into.programs.len() - 1) as u32
            }
        })
        .collect();
    for login in more.listed.iter_mut().chain(&mut more.candidates) {
        login.programs = login.programs.iter().filter_map(|index| indices.get(*index as usize).copied()).collect();
    }
    for (held, more) in [(&mut into.listed, more.listed), (&mut into.candidates, more.candidates)] {
        for login in more {
            if let Some(held) = held.iter_mut().find(|held| held.dir == login.dir) {
                let mut programs = std::mem::take(&mut held.programs);
                union(&mut programs, login.programs.clone());
                *held = login;
                held.programs = programs;
            } else {
                held.push(login);
            }
        }
    }
    union(&mut into.elsewhere, more.elsewhere);
}
