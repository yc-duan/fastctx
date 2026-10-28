//! Exact host-priced requests and the still-unidentified part of a process total.
use super::calls::Detail;
use super::ledger::{Book, Entry, Use};
use fastctx_types::usage::{Tokens, Usd};
use fastctx_types::usage_calls::{CallOutcome, CallPhase, CallSource, CallTiming};
use fastctx_types::{ResidentId, access::HubAccount, residents::About};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use std::{io, path::Path};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Meter {
    pub at: Timestamp,
    pub who: Option<HubAccount>,
    /// The other accounts it was used for alike (`Entry::uses`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub with: Vec<HubAccount>,
    pub session: String,
    pub process: String,
    pub since: Usd,
    pub total: Usd,
    pub paid: Usd,
    pub paid_dollars: f64,
    pub detail: Detail,
    pub key: Option<String>,
    pub usage: Option<Use>,
    pub unidentified: Vec<(Timestamp, Usd)>,
}

#[derive(Deserialize)]
pub(super) struct Priced {
    key: String,
    usd: f64,
    model: String,
    tokens: Tokens,
    timing: CallTiming,
    source: CallSource,
    turn: Option<String>,
    agent: Option<String>,
    #[serde(default)]
    effort: Option<String>,
}

impl Meter {
    pub fn balances(&self) -> Vec<Entry> {
        self.unidentified
            .iter()
            .map(|(at, usd)| {
                let mut call = self.detail.clone();
                call.turn = None;
                call.agent = None;
                call.effort = None;
                call.source = CallSource::Background;
                call.observed = false;
                call.outcome = CallOutcome::Unknown;
                call.phase = CallPhase::Unknown;
                call.timing.ended_at = *at;
                Entry::Summary {
                    at: *at,
                    who: self.who.clone(),
                    with: self.with.clone(),
                    usage: Use {
                        model: String::new(),
                        usd: Some(*usd),
                        tokens: Tokens::default(),
                        kinds: None,
                        call: Some(call),
                    },
                }
            })
            .collect()
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn record(
    book: &mut Book,
    path: &Path,
    id: &ResidentId,
    about: &About,
    at: Timestamp,
    process: String,
    since: Usd,
    total: Usd,
    priced: Option<Priced>,
) -> io::Result<bool> {
    let Some(session) = &about.session else { return Ok(false) };
    let identity = (session.clone(), Some(process.clone()));
    let key = priced.as_ref().map(|p| format!("claude:{session}:{process}:{}", p.key));
    if key.as_ref().is_some_and(|key| book.keys.contains(key)) {
        return Ok(false);
    }
    let previous = book.meters.get(&identity);
    if priced.is_none() && previous.is_some_and(|old| old.total >= total) {
        return Ok(false);
    }
    let mut meter = previous.cloned().unwrap_or_else(|| Meter {
        at,
        who: super::who(about),
        with: super::with(about),
        session: session.clone(),
        process,
        since,
        total: since,
        paid: Usd::ZERO,
        paid_dollars: 0.0,
        key: None,
        usage: None,
        unidentified: Vec::new(),
        detail: Detail {
            conversation: id.clone(),
            acting: about.acting.clone(),
            turn: None,
            source: CallSource::Background,
            agent: None,
            effort: None,
            timing: CallTiming { started_at: None, first_at: None, ended_at: at, approximate: false },
            outcome: CallOutcome::Unknown,
            phase: CallPhase::Unknown,
            observed: false,
        },
    });
    meter.at = at;
    if total > meter.total {
        meter.unidentified.push((at, total.saturating_sub(meter.total)));
        meter.total = total;
    }
    meter.detail.timing.ended_at = at;
    meter.key = key;
    meter.usage = None;
    if let Some(priced) = priced {
        if !priced.usd.is_finite() || priced.usd < 0.0 {
            return Err(io::Error::other("invalid request cost"));
        }
        let dollars = meter.paid_dollars + priced.usd;
        let paid = Usd::from_dollars(dollars).ok_or_else(|| io::Error::other("invalid request total"))?;
        let usd = paid.saturating_sub(meter.paid);
        let mut remaining = usd;
        let started = priced.timing.started_at.unwrap_or(priced.timing.ended_at);
        for (sampled, unknown) in &mut meter.unidentified {
            if *sampled < started {
                continue;
            }
            let take = Usd(remaining.0.min(unknown.0));
            *unknown = unknown.saturating_sub(take);
            remaining = remaining.saturating_sub(take);
        }
        meter.unidentified.retain(|(_, usd)| *usd != Usd::ZERO);
        // A receipt newer than every sample extends the total until a later
        // cumulative sample catches up; it cannot spend an older unknown sum.
        meter.total = meter.total.saturating_add(remaining);
        meter.paid = paid;
        meter.paid_dollars = dollars;
        meter.usage = Some(Use {
            kinds: Some(super::claude::kinds(&priced.model, &priced.tokens, usd)),
            model: priced.model,
            usd: Some(usd),
            tokens: priced.tokens,
            call: Some(Detail {
                conversation: id.clone(),
                acting: about.acting.clone(),
                turn: priced.turn,
                source: priced.source,
                agent: priced.agent,
                effort: priced.effort,
                timing: priced.timing,
                outcome: CallOutcome::Completed,
                phase: CallPhase::Finished,
                observed: true,
            }),
        });
    }
    meter.total = meter.total.max(meter.since.saturating_add(meter.paid));
    book.commit(path, Entry::Meter { meter: Box::new(meter) })?;
    Ok(true)
}
