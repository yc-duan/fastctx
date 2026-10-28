//! Claude's process-scoped cumulative cost and the requests it pays for.
use super::calls::Detail;
use super::ledger::{Entry, Use};
use crate::Node;
use fastctx_types::ResidentId;
use fastctx_types::residents::About;
use fastctx_types::session::RequestUsage;
use fastctx_types::usage::{KindsUsd, Tokens, Usd};
use fastctx_types::usage_calls::{CallOutcome, CallPhase, CallSource, CallTiming};
use jiff::Timestamp;
use serde::Deserialize;
use serde_json::Value;
use std::io;
use std::path::Path;

#[derive(Deserialize)]
pub(super) struct Legacy {
    pub usd: Usd,
}
#[derive(Deserialize)]
struct Counted {
    process: String,
    since: f64,
    #[serde(default)]
    steps: Vec<Step>,
    #[serde(default)]
    meter: bool,
    #[serde(default)]
    priced: Option<super::meter::Priced>,
    #[serde(default)]
    detail_key: Option<String>,
    #[serde(default)]
    account_at: Option<Timestamp>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Step {
    Current {
        usage: Option<RequestUsage>,
        model: String,
        turn: String,
        agent: Option<String>,
        #[serde(default)]
        effort: Option<String>,
        timing: CallTiming,
        outcome: CallOutcome,
        phase: CallPhase,
    },
    Legacy(RequestUsage),
}
impl Step {
    fn usage(&self) -> Option<&RequestUsage> {
        match self {
            Self::Current { usage, .. } => usage.as_ref(),
            Self::Legacy(usage) => Some(usage),
        }
    }
}

pub(super) fn measured(
    node: &Node,
    path: &Path,
    id: &ResidentId,
    about: &About,
    usage: &Value,
    at: Timestamp,
) -> io::Result<bool> {
    let windows = usage.get("rateLimits").and_then(Value::as_array).map(|rows| super::readings::claude_windows(rows));
    let counted = usage
        .get("counted")
        .map(|value| serde_json::from_value::<Counted>(value.clone()).map_err(io::Error::other))
        .transpose()?;
    let total = usage.pointer("/cost/usd").and_then(Value::as_f64).and_then(Usd::from_dollars);
    let legacy = if counted.is_none() {
        let mut legacy = node.usage.legacy.lock().expect("never poisoned");
        if legacy.is_none() {
            *legacy = Some(match std::fs::read(super::root(node).join("claude-costs.json")) {
                Ok(bytes) => serde_json::from_slice(&bytes).map_err(io::Error::other)?,
                Err(e) if e.kind() == io::ErrorKind::NotFound => Default::default(),
                Err(e) => return Err(e),
            });
        }
        about.session.as_ref().and_then(|s| legacy.as_ref().and_then(|m| m.get(s))).map(|v| v.usd)
    } else {
        None
    };
    super::with_book(node, path, |book| {
        let mut changed = false;
        if let Some(windows) = windows {
            let account = book.account_at(counted.as_ref().and_then(|c| c.account_at).unwrap_or(about.born_at));
            changed |= super::readings::record(book, path, windows, at, account)?;
        }
        if counted.as_ref().is_some_and(|c| c.meter) {
            let c = counted.expect("meter report");
            let since = Usd::from_dollars(c.since).ok_or_else(|| io::Error::other("invalid process baseline"))?;
            changed |= super::meter::record(
                book,
                path,
                id,
                about,
                at,
                c.process.clone(),
                since,
                total.unwrap_or(since),
                c.priced,
            )?;
            if let Some(key) = c.detail_key {
                for usage in divide(Usd::ZERO, &c.steps, id, about, at) {
                    book.commit(
                        path,
                        Entry::Tokens {
                            at,
                            who: super::who(about),
                            with: super::with(about),
                            model: usage.model,
                            tokens: usage.tokens,
                            call: usage.call,
                            key: Some(format!(
                                "claude-step:{}:{}:{key}",
                                about.session.as_deref().unwrap_or_default(),
                                c.process
                            )),
                        },
                    )?;
                    changed = true;
                }
            }
            return Ok(changed);
        }
        let (Some(session), Some(total)) = (&about.session, total) else {
            return Ok(changed);
        };
        let process = counted.as_ref().map(|c| c.process.clone());
        let identity = (session.clone(), process.clone());
        if book.seen.contains(&(identity.clone(), at)) {
            return Ok(changed);
        }
        let since = counted
            .as_ref()
            .map(|c| Usd::from_dollars(c.since).ok_or_else(|| io::Error::other("invalid Claude process baseline")))
            .transpose()?;
        let before = book.totals.get(&identity).map(|(_, total)| *total).or(since).or(legacy).unwrap_or(Usd::ZERO);
        let rise = if process.is_none() && total < before { total } else { total.saturating_sub(before) };
        let steps = counted.map(|c| c.steps).unwrap_or_default();
        if rise == Usd::ZERO && steps.is_empty() {
            return Ok(changed);
        }
        let mut uses = divide(rise, &steps, id, about, at);
        if process.is_none() {
            for usage in &mut uses {
                usage.kinds = None;
                usage.call = None;
            }
        }
        book.commit(
            path,
            Entry::Spent {
                at,
                who: super::who(about),
                with: super::with(about),
                session: session.clone(),
                process,
                total: if since.is_some() { total.max(before) } else { total },
                uses,
            },
        )?;
        Ok(true)
    })
}

/// Integer shares sum exactly to the host's amount; rounding belongs to the largest weight.
pub(super) fn shares(total: Usd, weights: &[f64]) -> Vec<Usd> {
    if weights.is_empty() {
        return Vec::new();
    }
    let sum: f64 = weights.iter().sum();
    let largest = weights.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).map_or(0, |(i, _)| i);
    let mut shares = vec![Usd::ZERO; weights.len()];
    let mut left = total.0;
    if sum > 0.0 {
        for (i, weight) in weights.iter().enumerate() {
            if i == largest {
                continue;
            }
            let part = ((total.0 as f64 * weight / sum).floor() as u64).min(left);
            shares[i] = Usd(part);
            left -= part;
        }
    }
    shares[largest] = Usd(left);
    shares
}

pub(super) fn kinds(model: &str, tokens: &Tokens, usd: Usd) -> KindsUsd {
    let weights = fastctx_hosts::claude::pricing::weights(model, tokens);
    let amounts = shares(usd, &weights);
    KindsUsd { input: amounts[0], cache_read: amounts[1], cache_write: amounts[2], output: amounts[3] }
}

fn divide(total: Usd, steps: &[Step], id: &ResidentId, about: &About, at: Timestamp) -> Vec<Use> {
    let tokens: Vec<Tokens> = steps
        .iter()
        .map(|s| {
            s.usage()
                .map(|s| Tokens {
                    input: s.input_tokens,
                    cache_read: s.cache_read_input_tokens,
                    cache_write: s.cache_creation_input_tokens,
                    output: s.output_tokens,
                    reasoning: 0,
                })
                .unwrap_or_default()
        })
        .collect();
    let weights: Vec<f64> = steps
        .iter()
        .zip(&tokens)
        .map(|(s, t)| s.usage().map_or(0.0, |u| fastctx_hosts::claude::pricing::weight(&u.model, t).max(f64::EPSILON)))
        .collect();
    let observed = steps.iter().any(|s| s.usage().is_some());
    let amounts = shares(if observed { total } else { Usd::ZERO }, &weights);
    let mut uses = Vec::new();
    for ((step, tokens), usd) in steps.iter().zip(tokens).zip(amounts) {
        let (model, call) = match step {
            Step::Legacy(usage) => (usage.model.clone(), None),
            Step::Current { usage, model, turn, agent, effort, timing, outcome, phase } => (
                usage.as_ref().map_or_else(|| model.clone(), |u| u.model.clone()),
                Some(Detail {
                    conversation: id.clone(),
                    acting: about.acting.clone(),
                    turn: Some(turn.clone()),
                    source: if agent.is_some() { CallSource::Agent } else { CallSource::Main },
                    agent: agent.clone(),
                    effort: effort.clone(),
                    timing: timing.clone(),
                    outcome: *outcome,
                    phase: *phase,
                    observed: usage.is_some(),
                }),
            ),
        };
        uses.push(Use {
            kinds: Some(kinds(&model, &tokens, usd)),
            model,
            usd: step.usage().map(|_| usd),
            tokens,
            call,
        });
    }
    if !observed && (total != Usd::ZERO || steps.is_empty()) {
        uses.push(Use {
            model: String::new(),
            usd: Some(total),
            tokens: Tokens::default(),
            kinds: Some(KindsUsd { input: total, ..Default::default() }),
            call: Some(Detail {
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
            }),
        });
    }
    uses
}

pub(super) fn add_kinds(to: &mut KindsUsd, from: KindsUsd) {
    to.input = to.input.saturating_add(from.input);
    to.cache_read = to.cache_read.saturating_add(from.cache_read);
    to.cache_write = to.cache_write.saturating_add(from.cache_write);
    to.output = to.output.saturating_add(from.output);
}
