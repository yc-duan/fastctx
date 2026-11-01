//! Large legacy frames are independent idempotent members followed by their
//! terminal metadata. The durable transport retains the frame until all commits
//! succeed; a restart replays missing members before advancing the job state.

use super::{snapshot, sql};
use fastctx_storage::Database;
use fastctx_types::run_facts::JobFacts;
use std::io;

const MEMBERS: usize = 16;

pub(super) fn receive(
    database: &Database,
    conversation: &str,
    update: &JobFacts,
    publish: impl FnMut(JobFacts),
) -> io::Result<bool> {
    receive_at(database, conversation, update, fastctx_storage::Priority::Normal, publish)
}

pub(super) fn receive_at(
    database: &Database,
    conversation: &str,
    update: &JobFacts,
    priority: fastctx_storage::Priority,
    mut publish: impl FnMut(JobFacts),
) -> io::Result<bool> {
    let Some(mut incoming) =
        database.read_now(|db| sql::additions(db, conversation, update)).map_err(io::Error::other)?
    else {
        return Ok(false);
    };
    let writes = std::mem::take(&mut incoming.writes);
    let steps = std::mem::take(&mut incoming.steps);
    let visits = std::mem::take(&mut incoming.visited);
    let count = writes.len().max(steps.len()).max(visits.len()).div_ceil(MEMBERS);
    let mut any = false;
    let mut notifications = Vec::new();
    let mut template = incoming.clone();
    template.state = None;
    template.output = None;
    template.notes = None;
    template.finished_at = None;
    template.record = None;
    template.script = None;
    template.started_at = None;
    for index in 0..count {
        let mut part = template.clone();
        part.writes = writes.iter().skip(index * MEMBERS).take(MEMBERS).cloned().collect();
        part.steps = steps.iter().skip(index * MEMBERS).take(MEMBERS).cloned().collect();
        part.visited = visits.iter().skip(index * MEMBERS).take(MEMBERS).cloned().collect();
        let saved = part.clone();
        let scope = conversation.to_string();
        let bytes = sql::encode(&part).map_err(io::Error::other)?.len();
        let changed = database
            .write_now_at(priority, bytes, move |db| {
                let Some(mut head) = sql::head(db, &scope, &saved.call_id)? else { return Ok(None) };
                snapshot::validate(&head, &saved)?;
                let before = db.total_changes();
                if head.facts.turn.is_none() && saved.turn.is_some() {
                    head.facts.turn = saved.turn.clone();
                    sql::save(db, &scope, &head)?;
                }
                sql::writes(db, &scope, &saved.call_id, &saved.writes)?;
                sql::steps(db, &scope, &saved.call_id, &saved.steps)?;
                sql::visits(db, &scope, &saved.call_id, &saved.visited)?;
                let changed = db.total_changes() != before;
                if changed {
                    sql::changed(db, &scope)?;
                }
                if !changed {
                    return Ok(None);
                }
                let prior = sql::job(db, &scope, &saved)?;
                let buffered = prior.is_none() && saved.sequence == 0;
                let mut notice = saved;
                if let Some(prior) = prior {
                    notice.sequence = prior.sequence;
                    notice.state = prior.state;
                    notice.output = prior.output;
                    notice.notes = prior.notes;
                    notice.finished_at = prior.finished_at;
                } else {
                    notice.sequence = 0;
                    if notice.primary {
                        notice.state = Some(head.facts.state);
                        notice.output = head.facts.output;
                    }
                }
                Ok(Some((notice, buffered)))
            })
            .map_err(io::Error::other)?;
        if let Some((notice, buffered)) = changed {
            any = true;
            if buffered {
                notifications.push(notice);
            } else {
                publish(notice);
            }
        }
    }
    let mut saved = incoming.clone();
    let staged = saved
        .notes
        .take()
        .map(|notes| crate::record_payloads::stage(database, conversation, sql::encode(&notes)?, priority))
        .transpose()
        .map_err(io::Error::other)?;
    let scope = conversation.to_string();
    let bytes = sql::encode(&saved).map_err(io::Error::other)?.len();
    let changed = database
        .write_now_at(priority, bytes, move |db| {
            let changed = snapshot::receive_in(db, &scope, &saved, staged.as_ref().map(|staged| &staged.reference))?;
            if let Some(staged) = &staged {
                crate::record_payloads::finish(db, &scope, staged)?;
            }
            Ok(changed)
        })
        .map_err(io::Error::other)?;
    any |= changed;
    if any {
        // Member pages carry the previously committed metadata sequence, never
        // consume this frame's sequence before its final state is durable.
        // Sequence zero has no predecessor and is buffered until this commit.
        publish(incoming);
        for notification in notifications {
            publish(notification);
        }
    }
    Ok(any)
}

/// Store snapshot members before its final state, in independent bounded commits.
pub(super) fn keep_members(
    database: &Database,
    conversation: &str,
    call: &str,
    members: sql::Members,
) -> io::Result<()> {
    let count = members.writes.len().max(members.steps.len()).max(members.visited.len()).div_ceil(MEMBERS);
    for index in 0..count {
        let writes: Vec<_> = members.writes.iter().skip(index * MEMBERS).take(MEMBERS).cloned().collect();
        let steps: Vec<_> = members.steps.iter().skip(index * MEMBERS).take(MEMBERS).cloned().collect();
        let visited: Vec<_> = members.visited.iter().skip(index * MEMBERS).take(MEMBERS).cloned().collect();
        let bytes = sql::encode(&(&writes, &steps, &visited)).map_err(io::Error::other)?.len();
        let (scope, call) = (conversation.to_string(), call.to_string());
        database
            .write_now(bytes, move |db| {
                let before = db.total_changes();
                sql::writes(db, &scope, &call, &writes)?;
                sql::steps(db, &scope, &call, &steps)?;
                sql::visits(db, &scope, &call, &visited)?;
                if db.total_changes() != before {
                    sql::changed(db, &scope)?;
                }
                Ok(())
            })
            .map_err(io::Error::other)?;
    }
    Ok(())
}
