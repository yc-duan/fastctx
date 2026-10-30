//! Review deltas compare content, not a remote event's time or its original turn.

use crate::Node;
use fastctx_types::ResidentId;
use fastctx_types::session::{Happened, Speaker};
use fastctx_types::turn_evidence::TurnEvidence;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io;

#[derive(Default, Serialize, Deserialize)]
pub(crate) struct Seen(pub BTreeMap<String, String>);

#[derive(Serialize)]
pub(crate) struct Delta {
    pub turns: Vec<TurnEvidence>,
    pub unavailable: Vec<Unavailable>,
    pub jobs: Vec<ObservedJob>,
}

#[derive(Serialize)]
pub(crate) struct ObservedJob {
    turn: Option<String>,
    id: fastctx_types::JobId,
    machine: fastctx_types::MachineName,
    script: String,
    started_at: jiff::Timestamp,
    exit: Option<fastctx_types::run::Exited>,
    end: Option<fastctx_types::run::Ended>,
    remaining: Option<u32>,
    output: String,
    output_ended_at: Option<jiff::Timestamp>,
    output_lost: Option<fastctx_types::run::OutputLost>,
    full_record: String,
}

#[derive(Serialize)]
pub(crate) struct Unavailable {
    turn: Option<String>,
    call: String,
    reason: String,
    source: String,
}

struct Comparing<'a> {
    previous: &'a Seen,
    current: Seen,
}

impl Comparing<'_> {
    fn changed(&mut self, kind: &str, identity: &impl Serialize, value: &impl Serialize) -> io::Result<bool> {
        let key = fastctx_hands::files::content_hash(&serde_json::to_vec(&(kind, identity))?);
        let fingerprint = fastctx_hands::files::content_hash(&serde_json::to_vec(value)?);
        let changed = self.previous.0.get(&key) != Some(&fingerprint);
        self.current.0.insert(key, fingerprint);
        Ok(changed)
    }
}

/// The caller keeps this conversation's place alive. Each source is read once;
/// arrivals after a source was read remain unseen and enter the next review.
pub(crate) fn collect(node: &Node, resident: &ResidentId, previous: &Seen) -> io::Result<(Seen, Delta)> {
    let (about, _) = node.residents.about(resident).ok_or_else(|| io::Error::other("unknown conversation"))?;
    let directory = node.world.conversation(resident);
    let mut comparing = Comparing { previous, current: Seen::default() };
    let mut turns = BTreeMap::<Option<String>, TurnEvidence>::new();
    let mut unavailable = Vec::new();
    let mut jobs = Vec::new();
    for finished in crate::delivery::terminals(&directory)? {
        let mut end = finished.end;
        end.last_message = None;
        if let Some(error) = super::diagnostics::get(node, resident, &finished.turn)? {
            super::diagnostics::augment(&mut end, error);
        }
        let stop = super::classify(&end, super::source(node, resident, &finished.turn)?, about.host.agent.as_str());
        if comparing.changed("turn", &finished.turn, &(finished.at, finished.ended_at, &end, &stop))? {
            let facts = row(&mut turns, resident, Some(finished.turn));
            facts.started_at = end.facts.as_ref().and_then(|facts| facts.world_started_at);
            facts.last_activity_at = end.facts.as_ref().and_then(|facts| facts.world_last_activity_at);
            facts.ended_at = Some(finished.ended_at.unwrap_or(finished.at));
            facts.end_recorded_at = Some(finished.at);
            facts.end = Some(end);
            facts.stop = Some(stop);
        }
    }
    for action in super::records::actions(node, resident)? {
        if comparing.changed("action", &action, &action)? {
            row(&mut turns, resident, action.turn.clone()).actions.push(action);
        }
    }
    for observed in crate::run_facts::observations(&directory)? {
        if comparing.changed("call", &observed.call.call_id, &observed)? {
            let call = observed.call;
            if let Some(reason) = observed.unavailable {
                unavailable.push(Unavailable {
                    turn: call.facts.turn,
                    call: call.call_id,
                    reason,
                    source: call.source,
                });
            } else {
                row(&mut turns, resident, call.facts.turn.clone()).calls.push(call);
            }
        }
    }
    for job in super::records::jobs(node, resident)? {
        let job = ObservedJob {
            turn: job.record.turn,
            id: job.record.id,
            machine: job.record.machine,
            script: job.record.script,
            started_at: job.record.started_at,
            exit: job.record.exit,
            end: job.record.end,
            remaining: job.record.remaining,
            output: job.record.output,
            output_ended_at: job.record.output_ended_at,
            output_lost: job.record.output_lost,
            full_record: job.source,
        };
        if comparing.changed("job", &(&job.machine, &job.id), &job)? {
            jobs.push(job);
        }
    }
    for update in crate::run_facts::job_updates(&directory)? {
        if comparing.changed("job_update", &(&update.machine, &update.job, &update.call_id), &update)? {
            row(&mut turns, resident, update.turn.clone()).job_updates.push(update);
        }
    }
    for event in crate::delivery::history(&directory)? {
        let Happened::Heard { turn, from: Some(Speaker::World), .. } = &event.happened else { continue };
        if comparing.changed("notice", &event, &event)? {
            row(&mut turns, resident, turn.clone()).notices.push(event);
        }
    }
    Ok((comparing.current, Delta { turns: turns.into_values().collect(), unavailable, jobs }))
}

fn row<'a>(
    turns: &'a mut BTreeMap<Option<String>, TurnEvidence>,
    resident: &ResidentId,
    turn: Option<String>,
) -> &'a mut TurnEvidence {
    turns.entry(turn.clone()).or_insert_with(|| TurnEvidence {
        resident: resident.clone(),
        turn,
        started_at: None,
        ended_at: None,
        end_recorded_at: None,
        last_activity_at: None,
        end: None,
        stop: None,
        actions: Vec::new(),
        calls: Vec::new(),
        jobs: Vec::new(),
        job_updates: Vec::new(),
        notices: Vec::new(),
        waiting: Vec::new(),
        quota: None,
    })
}

impl Delta {
    pub(crate) fn summary(&self, full_path: &str) -> String {
        use std::fmt::Write;
        let mut summary = format!(
            "Complete evidence: {full_path}. Source fields in that JSON are historical provenance; read the copied evidence here.\n"
        );
        for facts in &self.turns {
            let _ = writeln!(
                summary,
                "Turn {}: {} actions, {} calls, {} job updates; stop {:?}; ended {:?}",
                facts.turn.as_deref().unwrap_or("unknown"),
                facts.actions.len(),
                facts.calls.len(),
                facts.job_updates.len(),
                facts.stop,
                facts.ended_at
            );
        }
        for job in &self.jobs {
            let command: String =
                job.script.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(240).collect();
            let _ = writeln!(
                summary,
                "{} job {} · {} · exit {:?} · end {:?} · remaining {:?}",
                job.machine, job.id, command, job.exit, job.end, job.remaining
            );
        }
        for missing in &self.unavailable {
            let _ = writeln!(
                summary,
                "Turn {} call {}: detailed facts unavailable: {}",
                missing.turn.as_deref().unwrap_or("unknown"),
                missing.call,
                missing.reason
            );
        }
        summary
    }
}
