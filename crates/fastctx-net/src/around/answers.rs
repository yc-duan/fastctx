//! Merge account-specific answers without losing the question's provenance.

mod agents;

use fastctx_types::AccountName;
use fastctx_types::observation::{AccountFailure, MachineSource, Observation, Unanswered, union};
use fastctx_types::remote::RemoteReply;
use fastctx_types::since::{MachineSince, SinceReply};
use fastctx_types::town::{Looked, LookedUp, MachineWho, RunningJob, WhoReply};
use jiff::Timestamp;

use super::{Asked, Heard};

pub struct Answers {
    pub source: MachineSource,
    replies: Vec<(AccountName, RemoteReply)>,
    unanswered: Vec<AccountFailure>,
}

/// One group per hub and machine key, in completion order within each group.
pub fn collect(heard: Vec<(Asked, Heard)>) -> Vec<Answers> {
    let mut groups: Vec<Answers> = Vec::new();
    for (asked, heard) in heard {
        let at = groups.iter().position(|group| group.source.same_machine(&asked.source)).unwrap_or_else(|| {
            groups.push(Answers { source: asked.source.clone(), replies: Vec::new(), unanswered: Vec::new() });
            groups.len() - 1
        });
        let group = &mut groups[at];
        match heard {
            Heard::Answered(reply) => group.replies.push((asked.account, reply)),
            Heard::Offline(since) => {
                group.unanswered.push(AccountFailure { account: asked.account, failure: Unanswered::Offline { since } })
            }
            Heard::NoAnswer(reason) => group
                .unanswered
                .push(AccountFailure { account: asked.account, failure: Unanswered::NoAnswer { reason } }),
        }
    }
    groups
}

impl Answers {
    fn unexpected(&mut self, account: AccountName) {
        self.unanswered.push(AccountFailure {
            account,
            failure: Unanswered::NoAnswer { reason: "it answered something else".into() },
        });
    }

    fn observation(&self) -> Option<Observation> {
        Some(Observation { source: self.source.clone(), unanswered: self.unanswered.clone() })
    }

    fn offline(&self) -> Option<Option<Timestamp>> {
        let mut times = self.unanswered.iter().map(|failure| match failure.failure {
            Unanswered::Offline { since } => Some(since),
            Unanswered::NoAnswer { .. } => None,
        });
        let first = times.next()??;
        times.all(|time| time == Some(first)).then_some(first)
    }

    pub fn missing(&self) -> Vec<String> {
        self.unanswered.iter().map(|failure| format!("{} ({failure})", self.source.written)).collect()
    }

    pub fn who(mut self, history_requested: bool) -> MachineWho {
        let mut merged: Option<WhoReply> = None;
        for (account, reply) in std::mem::take(&mut self.replies) {
            match reply {
                RemoteReply::Who(mut reply) => {
                    reply.machine = self.source.written.name.clone();
                    if history_requested && reply.history.is_none() {
                        self.unanswered.push(AccountFailure {
                            account,
                            failure: Unanswered::NoAnswer {
                                reason: "answered current activity without the requested history".into(),
                            },
                        });
                    }
                    for resident in &mut reply.residents {
                        resident.source = Some(self.source.clone());
                    }
                    for job in &mut reply.jobs {
                        job.source = Some(self.source.clone());
                    }
                    if let Some(history) = &mut reply.history {
                        self.stamp_history(history);
                    }
                    match &mut merged {
                        Some(merged) => merged.merge(reply),
                        None => merged = Some(reply),
                    }
                }
                _ => self.unexpected(account),
            }
        }
        let observation = self.observation();
        match merged {
            Some(reply) => MachineWho::Answered { reply, observation },
            None => match self.offline() {
                Some(since) => {
                    MachineWho::Offline { machine: self.source.written.name.clone(), since, last: None, observation }
                }
                // The machine is named beside the reason, which is then the
                // same for every machine that went unasked for one cause.
                None => MachineWho::NoAnswer {
                    machine: self.source.written.name.clone(),
                    reason: self.unanswered.iter().map(ToString::to_string).collect::<Vec<_>>().join("; "),
                    observation,
                },
            },
        }
    }

    fn stamp_history(&self, reply: &mut SinceReply) {
        for event in &mut reply.events {
            event.hub.get_or_insert_with(|| self.source.hub.clone());
            event.source = Some(self.source.clone());
        }
        for fact in &mut reply.undated {
            fact.hub = Some(self.source.hub.clone());
        }
    }

    pub fn since(mut self) -> (MachineSince, Option<SinceReply>) {
        let mut merged: Option<SinceReply> = None;
        for (account, reply) in std::mem::take(&mut self.replies) {
            match reply {
                RemoteReply::Since(mut reply) => {
                    reply.coverage.machine = self.source.written.name.clone();
                    self.stamp_history(&mut reply);
                    match &mut merged {
                        Some(merged) => merged.merge(reply),
                        None => merged = Some(reply),
                    }
                }
                _ => self.unexpected(account),
            }
        }
        let observation = self.observation();
        let machine = self.source.written.name.clone();
        let status = match &merged {
            Some(reply) => MachineSince::Answered { reply: reply.coverage.clone(), observation },
            None => match self.offline() {
                Some(since) => MachineSince::Offline { machine, since, observation },
                None => MachineSince::NoAnswer { machine, reason: self.missing().join("; "), observation },
            },
        };
        (status, merged)
    }

    pub fn look(mut self, into: &mut LookedUp) {
        let mut found: Vec<(Option<Timestamp>, Looked)> = Vec::new();
        let mut jobs: Vec<RunningJob> = Vec::new();
        for (account, reply) in std::mem::take(&mut self.replies) {
            match reply {
                RemoteReply::Looked { looked, jobs: more, read_at } => {
                    if let Some(mut looked) = looked {
                        match &mut looked {
                            Looked::Resident(detail) => {
                                detail.resident.source = Some(self.source.clone());
                                for job in &mut detail.jobs {
                                    job.source = Some(self.source.clone());
                                }
                            }
                            Looked::Job { source, .. } => *source = Some(self.source.clone()),
                        }
                        if let Some((at, held)) = found.iter_mut().find(|(_, held)| held.id() == looked.id()) {
                            // Old peers supply no read time; completion order is then the only observation available.
                            if read_at.zip(*at).is_none_or(|(new, old)| new >= old) {
                                *held = looked;
                                *at = read_at;
                            }
                        } else {
                            found.push((read_at, looked));
                        }
                    }
                    for mut job in more {
                        job.source = Some(self.source.clone());
                        if let Some(held) = jobs.iter_mut().find(|held| held.job == job.job) {
                            *held = job;
                        } else {
                            jobs.push(job);
                        }
                    }
                }
                _ => self.unexpected(account),
            }
        }
        into.found.extend(found.into_iter().map(|(_, looked)| looked));
        into.jobs.extend(jobs);
        union(&mut into.unasked, self.missing());
    }

    /// Presence and missing account answers are independent facts.
    pub fn find(mut self) -> (MachineSource, bool, Vec<String>) {
        let mut found = false;
        for (account, reply) in std::mem::take(&mut self.replies) {
            match reply {
                RemoteReply::Found { lives_here } => found |= lives_here,
                _ => self.unexpected(account),
            }
        }
        let missing = self.missing();
        (self.source, found, missing)
    }
}

/// The same hub observation in standalone history and history carried by Who.
pub fn presence(
    source: &MachineSource,
    online: bool,
    at: Option<Timestamp>,
    since: Timestamp,
) -> Option<fastctx_types::since::SinceEvent> {
    use fastctx_types::since::{SinceEvent, SinceKind, SinceTown};
    let at = at.filter(|at| *at > since)?;
    let bytes = format!("{}\0{}\0{at}\0{online}", source.hub, source.key);
    let digest: String =
        crate::sign::sha256(bytes.as_bytes()).iter().take(10).map(|byte| format!("{byte:02x}")).collect();
    Some(SinceEvent {
        hub: Some(source.hub.clone()),
        id: format!("presence-{digest}"),
        source: Some(source.clone()),
        at,
        machine: source.written.name.clone(),
        town: SinceTown::Unrecorded { recorded_in: Vec::new() },
        sources: Vec::new(),
        what: if online { SinceKind::MachineConnected } else { SinceKind::MachineOffline },
    })
}
