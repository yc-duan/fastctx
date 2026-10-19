//! A login's use as one reader may see it: everyone's to an account that may
//! work on this machine, its own alone to an account the login assists,
//! which does not see the login's account's windows either.

use std::collections::BTreeMap;
use std::io;
use std::path::Path;

use fastctx_types::access::HubAccount;
use fastctx_types::usage::{
    KindsUsd, ModelUse, Tokens, UsageHour, UsagePerson, UsageReading, UsageReport, UsageShare, UsageSpan, UsageSum,
    UsageWeek, Usd, WeekUse,
};
use jiff::{SignedDuration, Timestamp};

use super::{Entry, WEEK_SECONDS, Week};
use crate::Node;

/// What a token of each kind costs, against one of uncached input, at
/// Anthropic's list prices for every model: a cache read a tenth, a cache
/// write a quarter more, output five times. They split what the ledger
/// says a week cost by kind; the ledger itself says what it cost in all.
const READ: f64 = 0.1;
const WRITE: f64 = 1.25;
const OUTPUT: f64 = 5.0;

/// The same at OpenAI's list prices for its GPT-5 models, which share out
/// a week's window among those who used a Codex login, which keeps no
/// cost: a cached input token a tenth of an uncached one, output, its
/// reasoning among it, eight times.
const CODEX_READ: f64 = 0.1;
const CODEX_OUTPUT: f64 = 8.0;

/// Resets this close are one: Codex says a window's reset a few seconds
/// apart from one reading to the next.
const SAME_RESET: SignedDuration = SignedDuration::from_secs(120);

/// `agent`'s login `dir` here, as `only` may see it: everyone's use, or the
/// use of that one account alone.
pub(crate) fn report(node: &Node, agent: &str, dir: &Path, only: Option<&HubAccount>) -> io::Result<UsageReport> {
    let ledger = super::ledger(node, agent, dir);
    let now = Timestamp::now();
    let since = now - SignedDuration::from_hours(24 * super::KEPT_DAYS);
    let day = now - SignedDuration::from_hours(24);
    let week = super::week(node, &ledger, now);
    let in_week = |at: Timestamp| at >= week.start && at < week.end.max(now);
    let entries = super::entries(&ledger, since)?;
    let spans = spans(&entries, week);
    let span_of = |at: Timestamp| {
        let index = spans.partition_point(|span| span.start <= at).checked_sub(1)?;
        (at < spans[index].end).then_some(index)
    };
    // What the account used of each week's window, to share out among
    // those who used it; an account the login only assists is told none.
    let mut reached: Vec<Option<u32>> = vec![None; spans.len()];
    let mut weights: BTreeMap<(usize, u32), f64> = BTreeMap::new();
    let mut people: Vec<Option<HubAccount>> = Vec::new();
    let mut person = |who: Option<HubAccount>| -> u32 {
        let index = people.iter().position(|known| *known == who).unwrap_or_else(|| {
            people.push(who);
            people.len() - 1
        });
        u32::try_from(index).expect("fewer people than u32 counts")
    };
    let mut hours: BTreeMap<(i64, u32), UsageSum> = BTreeMap::new();
    let mut used: BTreeMap<(usize, u32), UsageSum> = BTreeMap::new();
    let mut weekly: BTreeMap<u32, UsageSum> = BTreeMap::new();
    let mut daily: BTreeMap<u32, UsageSum> = BTreeMap::new();
    let mut went: BTreeMap<u32, Tally> = BTreeMap::new();
    let mut all = Tally::default();
    let mut readings = Vec::new();
    for entry in entries {
        let (at, who, tokens, usd, model) = match entry {
            Entry::Cost { at, who, model, usd } => (at, who, None, Some(usd), model),
            Entry::Tokens { at, who, model, tokens } => (at, who, Some(tokens), None, model),
            Entry::Reading { at, windows } => {
                if only.is_some() {
                    continue;
                }
                if let Some(span) = span_of(at)
                    && let Some(window) = windows.iter().find(|window| {
                        window.minutes == Some(10_080) && window.resets_at.is_some_and(|reset| same(reset, spans[span].reset))
                    })
                {
                    reached[span] = Some(reached[span].unwrap_or(0).max(window.used_tenths));
                }
                if in_week(at) {
                    readings.push(UsageReading { at, windows });
                }
                continue;
            }
        };
        if only.is_some_and(|only| who.as_ref() != Some(only)) {
            continue;
        }
        let index = person(who);
        let count = tokens.as_ref().map_or(0, Tokens::total);
        hours.entry((hour(at), index)).or_default().add(count, usd);
        if let Some(span) = span_of(at) {
            used.entry((span, index)).or_default().add(count, usd);
            *weights.entry((span, index)).or_default() += weight(agent, tokens.as_ref(), usd);
        }
        if at >= day {
            daily.entry(index).or_default().add(count, usd);
        }
        if in_week(at) {
            weekly.entry(index).or_default().add(count, usd);
            went.entry(index).or_default().add(&model, tokens.as_ref(), usd);
            all.add(&model, tokens.as_ref(), usd);
        }
    }
    Ok(UsageReport {
        people: people
            .into_iter()
            .enumerate()
            .map(|(index, account)| {
                let index = index as u32;
                UsagePerson {
                    account,
                    week: weekly.get(&index).copied().unwrap_or_default(),
                    day: Some(daily.get(&index).copied().unwrap_or_default()),
                    went: Some(went.remove(&index).unwrap_or_default().went()),
                }
            })
            .collect(),
        hours: hours
            .into_iter()
            .map(|((at, person), sum)| UsageHour {
                at: Timestamp::from_second(at).expect("an hour within a year of now"),
                person,
                sum,
            })
            .collect(),
        week: UsageWeek { start: Some(week.start), end: Some(week.end), window: week.window, went: all.went() },
        readings,
        weeks: spans
            .iter()
            .enumerate()
            .map(|(index, span)| {
                let all: f64 = weights.range((index, 0)..=(index, u32::MAX)).map(|(_, weight)| weight).sum();
                let part = |person: u32| {
                    let reached = reached[index]?;
                    let weight = weights.get(&(index, person)).copied().unwrap_or(0.0);
                    Some(if all > 0.0 { (f64::from(reached) * weight / all).round() as u32 } else { 0 })
                };
                UsageSpan {
                    start: span.start,
                    end: span.end,
                    used_tenths: reached[index],
                    used: used
                        .range((index, 0)..=(index, u32::MAX))
                        .map(|((_, person), sum)| UsageShare { person: *person, sum: *sum, quota_tenths: part(*person) })
                        .collect(),
                }
            })
            .collect(),
    })
}

/// One week of the login's own window: when it began and ended, and the
/// reset its readings named.
struct Span {
    start: Timestamp,
    end: Timestamp,
    reset: Timestamp,
}

fn same(a: Timestamp, b: Timestamp) -> bool {
    a.duration_since(b).abs() <= SAME_RESET
}

/// What one ledger entry weighs in sharing out a week's window: Claude
/// Code's dollars; Codex's tokens at list-price weights.
fn weight(agent: &str, tokens: Option<&Tokens>, usd: Option<Usd>) -> f64 {
    if agent == "claude" {
        return usd.map_or(0.0, |usd| usd.0 as f64);
    }
    tokens.map_or(0.0, |tokens| {
        tokens.input as f64 + tokens.cache_read as f64 * CODEX_READ + (tokens.output + tokens.reasoning) as f64 * CODEX_OUTPUT
    })
}

/// The login's weeks over the year, oldest first, the current one last: a
/// week back from each reset of its seven-day window a reading had, each
/// ended where the next began if that was sooner, as when another account
/// logged in. Only weeks a reading recorded: none before the first.
fn spans(entries: &[Entry], current: Week) -> Vec<Span> {
    if !current.window {
        return Vec::new();
    }
    let mut resets: Vec<Timestamp> = entries
        .iter()
        .filter_map(|entry| match entry {
            Entry::Reading { windows, .. } => {
                windows.iter().find(|window| window.minutes == Some(10_080)).and_then(|window| window.resets_at)
            }
            _ => None,
        })
        .filter(|reset| *reset < current.end && !same(*reset, current.end))
        .collect();
    resets.sort();
    resets.dedup_by(|later, earlier| same(*later, *earlier));
    let week = SignedDuration::from_secs(WEEK_SECONDS);
    let mut spans: Vec<Span> = resets.into_iter().map(|reset| Span { start: reset - week, end: reset, reset }).collect();
    spans.push(Span { start: current.start, end: current.end, reset: current.end });
    for index in 1..spans.len() {
        let next = spans[index].start;
        if spans[index - 1].end > next {
            spans[index - 1].end = next;
        }
    }
    spans.retain(|span| span.start < span.end);
    spans
}

/// What a week's use went to, as it is counted.
#[derive(Default)]
struct Tally {
    models: BTreeMap<String, UsageSum>,
    tokens: Tokens,
    spent: Option<Usd>,
}

impl Tally {
    fn add(&mut self, model: &str, tokens: Option<&Tokens>, usd: Option<Usd>) {
        let count = tokens.map_or(0, Tokens::total);
        self.models.entry(model.to_string()).or_default().add(count, usd);
        if let Some(tokens) = tokens {
            self.tokens.add(tokens);
        }
        if let Some(usd) = usd {
            self.spent = Some(self.spent.unwrap_or(Usd::ZERO).saturating_add(usd));
        }
    }

    fn went(self) -> WeekUse {
        let mut models: Vec<ModelUse> = self.models.into_iter().map(|(model, sum)| ModelUse { model, sum }).collect();
        models.sort_by(|a, b| b.sum.usd.cmp(&a.sum.usd).then(b.sum.tokens.cmp(&a.sum.tokens)));
        WeekUse { models, tokens: self.tokens, kinds_usd: self.spent.map(|spent| kinds(&self.tokens, spent)) }
    }
}

/// The start of the hour `at` is in, in seconds.
fn hour(at: Timestamp) -> i64 {
    at.as_second().div_euclid(3600) * 3600
}

/// What a week spent, split by kind of token at list prices.
fn kinds(tokens: &Tokens, spent: Usd) -> KindsUsd {
    let weights = [
        tokens.input as f64,
        tokens.cache_read as f64 * READ,
        tokens.cache_write as f64 * WRITE,
        (tokens.output + tokens.reasoning) as f64 * OUTPUT,
    ];
    let total: f64 = weights.iter().sum();
    if total <= 0.0 {
        return KindsUsd::default();
    }
    let share = |weight: f64| Usd((spent.0 as f64 * weight / total).round() as u64);
    KindsUsd {
        input: share(weights[0]),
        cache_read: share(weights[1]),
        cache_write: share(weights[2]),
        output: share(weights[3]),
    }
}
