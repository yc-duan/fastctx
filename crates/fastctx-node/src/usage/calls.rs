//! Request details are local financial records; conversation ownership is a separate check.
use super::ledger;
use crate::Node;
use crate::admission::seen::Looker;
use fastctx_types::ResidentId;
use fastctx_types::access::{Acting, HubAccount};
use fastctx_types::usage_calls::{
    CallCursor, CallOutcome, CallPhase, CallSource, CallTiming, UsageCall, UsageCallsPage, UsageCallsQuery,
};
use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io;
use std::path::Path;

pub(super) const DETAIL_DAYS: i64 = 30;

/// Ownership remains available after the conversation itself is deleted.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Detail {
    pub conversation: ResidentId,
    pub acting: Acting,
    pub turn: Option<String>,
    pub source: CallSource,
    pub agent: Option<String>,
    pub timing: CallTiming,
    pub outcome: CallOutcome,
    pub phase: CallPhase,
    pub observed: bool,
}

/// Admission has already selected this login. `only` restricts rows, while
/// `looker` independently controls the two private conversation identifiers.
pub(crate) fn calls(
    node: &Node,
    agent: &str,
    dir: &Path,
    only: Option<&HubAccount>,
    looker: Looker<'_>,
    query: &UsageCallsQuery,
) -> io::Result<UsageCallsPage> {
    let path = super::ledger(node, agent, dir);
    let since = Timestamp::now() - SignedDuration::from_hours(24 * DETAIL_DAYS);
    super::with_book(node, &path, |_| {
        let mut rows = Vec::new();
        for file in ledger::days(&path, since)?.into_iter().rev() {
            for entry in ledger::read(&file)? {
                if entry.at() < since {
                    continue;
                }
                // Row contents remain stable when expired neighbors are folded away.
                let identity = Sha256::digest(serde_json::to_vec(&entry).map_err(io::Error::other)?);
                for (item, (who, usage)) in entry.uses().into_iter().enumerate() {
                    if only.is_some_and(|only| who.as_ref() != Some(only)) {
                        continue;
                    }
                    let Some(detail) = usage.call else { continue };
                    let owns = looker.sees(node, &detail.acting);
                    let mut key = Sha256::new();
                    key.update(identity);
                    key.update((item as u64).to_be_bytes());
                    let cursor = CallCursor { at: entry.at(), key: format!("{:x}", key.finalize()) };
                    let row = UsageCall {
                        cursor,
                        who,
                        model: (detail.source != CallSource::Background || detail.observed).then_some(usage.model),
                        tokens: detail.observed.then_some(usage.tokens),
                        usd: usage.usd,
                        source: detail.source,
                        agent: detail.agent,
                        timing: detail.timing,
                        outcome: detail.outcome,
                        phase: detail.phase,
                        conversation: owns.then_some(detail.conversation),
                        turn: if owns { detail.turn } else { None },
                    };
                    // Filtering after redaction cannot be used to discover another conversation.
                    if query.person.as_ref().is_some_and(|p| p.account != row.who)
                        || query.model.as_ref().is_some_and(|m| row.model.as_ref() != Some(m))
                        || query.source.is_some_and(|s| s != row.source)
                        || query.outcome.is_some_and(|s| s != row.outcome)
                        || query.conversation.as_ref().is_some_and(|c| row.conversation.as_ref() != Some(c))
                        || query.turn.as_ref().is_some_and(|t| row.turn.as_ref() != Some(t))
                        || query.before.as_ref().is_some_and(|before| row.cursor >= *before)
                    {
                        continue;
                    }
                    rows.push(row);
                }
            }
            rows.sort_by(|a, b| b.cursor.cmp(&a.cursor));
            if rows.len() > 100 {
                break;
            }
        }
        let more = rows.len() > 100;
        rows.truncate(100);
        let next = more.then(|| rows.last().expect("full page").cursor.clone());
        Ok(UsageCallsPage { rows, next })
    })
}
