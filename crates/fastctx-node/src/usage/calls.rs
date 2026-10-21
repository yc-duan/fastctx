//! Request details are local financial records; conversation ownership is a separate check.
use super::ledger;
use crate::Node;
use crate::admission::seen::Looker;
use fastctx_types::ResidentId;
use fastctx_types::access::{Acting, HubAccount};
use fastctx_types::usage_calls::{
    CALLS_PAGE, CallCursor, CallOutcome, CallPhase, CallSource, CallTiming, UsageCall, UsageCallsPage, UsageCallsQuery,
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
    /// Older rows have none; leaving it out keeps their stored identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
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
    let kept = Timestamp::now() - SignedDuration::from_hours(24 * DETAIL_DAYS);
    let since = query.from.map_or(kept, |from| from.max(kept));
    // An entry is filed under its own day, so the days after the cursor, or
    // after the end of the range asked for, are not read.
    let day = |at: &Timestamp| at.strftime("%Y-%m-%d").to_string();
    let last = [query.before.as_ref().map(|before| day(&before.at)), query.until.as_ref().map(day)]
        .into_iter()
        .flatten()
        .min();
    let numbered = query.page.is_some();
    super::with_book(node, &path, |book| {
        let mut rows = Vec::new();
        let balances = book.balances();
        for file in ledger::days(&path, since)?.into_iter().rev() {
            if let Some(last) = &last
                && file.file_stem().and_then(|s| s.to_str()).is_some_and(|day| day > last.as_str())
            {
                continue;
            }
            let mut entries = ledger::read(&file)?;
            entries.extend(balances.iter().filter(|row| ledger::day_file(&path, row.at()) == file).cloned());
            for entry in entries {
                if entry.usage_at() < since || query.until.is_some_and(|until| entry.usage_at() >= until) {
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
                    let cursor = CallCursor { at: entry.usage_at(), key: format!("{:x}", key.finalize()) };
                    let named = detail.source != CallSource::Background || detail.observed;
                    let row = UsageCall {
                        cursor,
                        who,
                        model: named.then_some(usage.model),
                        effort: detail.effort.filter(|_| named),
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
                    if query.person.as_ref().is_some_and(|p| p.account != row.who && !(p.with_machine && row.who.is_none()))
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
            // Paged by a cursor, the days older than a page's worth are left unread.
            if !numbered && rows.len() > CALLS_PAGE {
                break;
            }
        }
        if let Some(page) = query.page {
            let total = rows.len() as u64;
            let start = (page as usize).saturating_mul(CALLS_PAGE).min(rows.len());
            let mut rows = rows.split_off(start);
            let more = rows.len() > CALLS_PAGE;
            rows.truncate(CALLS_PAGE);
            let next = more.then(|| rows.last().expect("full page").cursor.clone());
            return Ok(UsageCallsPage { rows, next, total: Some(total) });
        }
        let more = rows.len() > CALLS_PAGE;
        rows.truncate(CALLS_PAGE);
        let next = more.then(|| rows.last().expect("full page").cursor.clone());
        Ok(UsageCallsPage { rows, next, total: None })
    })
}
