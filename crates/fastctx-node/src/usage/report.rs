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

use super::ledger::{Reading, Use};
use super::{Entry, WEEK_SECONDS, Week};
use crate::Node;

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
    let oldest = now - SignedDuration::from_hours(24 * super::KEPT_DAYS);
    let since = Timestamp::from_second(oldest.as_second().div_euclid(86400) * 86400).expect("retained day");
    super::with_book(node, &ledger, |book| {
        let mut entries = book.history.entries(&ledger, since, now, book.reading.as_ref())?;
        entries.extend(book.balances().into_iter().filter(|row| row.at() >= since));
        summarize(agent, only, now, book.reading.as_ref(), entries)
    })
}

pub(super) fn summarize(
    agent: &str,
    only: Option<&HubAccount>,
    now: Timestamp,
    reading: Option<&Reading>,
    entries: Vec<Entry>,
) -> io::Result<UsageReport> {
    let day = now - SignedDuration::from_hours(24);
    let week = super::readings::week(reading, now);
    let in_week = |at: Timestamp| at >= week.start && at < week.end.max(now);
    let owners: Vec<_> = entries
        .iter()
        .filter_map(|row| match row {
            Entry::Account { at, account } => Some((*at, account.clone())),
            _ => None,
        })
        .collect();
    let current_owner = owners.iter().max_by_key(|(at, _)| *at).map(|(_, key)| key.clone());
    let owner_of = |row: &Entry| match row {
        Entry::Reading { at, account, .. } => account.clone().or_else(|| {
            owners
                .iter()
                .filter(|(since, _)| since <= at)
                .max_by_key(|(since, _)| *since)
                .or_else(|| owners.iter().min_by_key(|(since, _)| *since))
                .map(|(_, key)| key.clone())
        }),
        _ => None,
    };
    let spans = spans(&entries, week);
    let span_of = |at: Timestamp| {
        let index = spans.partition_point(|span| span.start <= at).checked_sub(1)?;
        (at < spans[index].end).then_some(index)
    };
    // What the account used of each week's window, to share out among
    // those who used it; an account the login only assists is told none.
    let mut reached: Vec<Option<u32>> = vec![None; spans.len()];
    let mut weights: BTreeMap<(usize, u32), f64> = BTreeMap::new();
    let mut people: Vec<Option<HubAccount>> = entries
        .iter()
        .flat_map(Entry::uses)
        .map(|(who, _)| who)
        .filter(|who| only.is_none_or(|only| who.as_ref() == Some(only)))
        .collect();
    people.sort_by_key(|who| who.as_ref().map(|who| (who.hub.to_string(), who.account.to_string())));
    people.dedup();
    let person = |who: &Option<HubAccount>| people.iter().position(|known| known == who).expect("known person") as u32;
    let mut hours: BTreeMap<(Timestamp, u32), UsageSum> = BTreeMap::new();
    let mut used: BTreeMap<(usize, u32), UsageSum> = BTreeMap::new();
    let mut weekly: BTreeMap<u32, UsageSum> = BTreeMap::new();
    let mut fable: BTreeMap<u32, UsageSum> = BTreeMap::new();
    let mut daily: BTreeMap<u32, UsageSum> = BTreeMap::new();
    let mut went: BTreeMap<u32, Tally> = BTreeMap::new();
    let mut all = Tally::default();
    let mut readings = Vec::new();
    let boundaries = boundaries(&spans, week);
    for entry in entries {
        let at = entry.at();
        if let Entry::Reading { windows, .. } = &entry {
            if only.is_some() {
                continue;
            }
            if let Some(span) = span_of(at)
                && owner_of(&entry) == spans[span].account
                && let Some(window) = windows.iter().find(|window| {
                    window.minutes == Some(10_080)
                        && window.resets_at.is_some_and(|reset| same(reset, spans[span].reset))
                })
            {
                reached[span] = Some(reached[span].unwrap_or(0).max(window.used_tenths));
            }
            if in_week(at) && owner_of(&entry) == current_owner {
                readings.push(UsageReading { at, windows: windows.clone() });
            }
            continue;
        }
        for (who, usage) in entry.uses() {
            if only.is_some_and(|only| who.as_ref() != Some(only)) {
                continue;
            }
            let index = person(&who);
            let count = usage.tokens.total();
            let hour = if matches!(entry, Entry::Summary { .. }) { at } else { bucket(at, &boundaries) };
            hours.entry((hour, index)).or_default().add(count, usage.usd);
            if let Some(span) = span_of(at) {
                used.entry((span, index)).or_default().add(count, usage.usd);
                *weights.entry((span, index)).or_default() += weight(agent, Some(&usage.tokens), usage.usd);
            }
            if at >= day {
                daily.entry(index).or_default().add(count, usage.usd);
            }
            if in_week(at) {
                weekly.entry(index).or_default().add(count, usage.usd);
                if fastctx_hosts::claude::pricing::is_fable(&usage.model)
                    && usage.call.as_ref().is_none_or(|call| call.observed)
                {
                    fable.entry(index).or_default().add(count, usage.usd);
                }
                went.entry(index).or_default().add(&usage);
                all.add(&usage);
            }
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
                    fable: (agent == "claude").then(|| fable.get(&index).copied().unwrap_or_default()),
                    went: Some(went.remove(&index).unwrap_or_default().went()),
                }
            })
            .collect(),
        hours: hours.into_iter().map(|((at, person), sum)| UsageHour { at, person, sum }).collect(),
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
                        .map(|((_, person), sum)| UsageShare {
                            person: *person,
                            sum: *sum,
                            quota_tenths: part(*person),
                        })
                        .collect(),
                }
            })
            .collect(),
    })
}

/// One week of the login's own window: when it began and ended, and the
/// reset its readings named.
pub(super) struct Span {
    pub start: Timestamp,
    pub end: Timestamp,
    pub reset: Timestamp,
    pub account: Option<String>,
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
        tokens.input as f64
            + tokens.cache_read as f64 * CODEX_READ
            + (tokens.output + tokens.reasoning) as f64 * CODEX_OUTPUT
    })
}

/// The login's weeks over the year, oldest first, the current one last: a
/// week back from each reset of its seven-day window a reading had, each
/// ended where the next began if that was sooner, as when another account
/// logged in. Only weeks a reading recorded: none before the first.
pub(super) fn spans(entries: &[Entry], current: Week) -> Vec<Span> {
    if !current.window {
        return Vec::new();
    }
    let mut owners: Vec<_> = entries
        .iter()
        .filter_map(|row| match row {
            Entry::Account { at, account } => Some((*at, account.clone())),
            _ => None,
        })
        .collect();
    owners.sort_by_key(|(at, _)| *at);
    let mut resets: Vec<(Timestamp, Option<String>, Timestamp)> = entries
        .iter()
        .filter_map(|entry| match entry {
            Entry::Reading { at, windows, account } => {
                let reset = windows.iter().find(|window| window.minutes == Some(10_080))?.resets_at?;
                let account = account.clone().or_else(|| {
                    owners
                        .iter()
                        .rev()
                        .find(|(since, _)| since <= at)
                        .or_else(|| owners.first())
                        .map(|(_, key)| key.clone())
                });
                Some((reset, account, *at))
            }
            _ => None,
        })
        .filter(|(reset, _, _)| *reset < current.end && !same(*reset, current.end))
        .collect();
    resets.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    resets.dedup_by(|later, earlier| later.1 == earlier.1 && same(later.0, earlier.0));
    resets.sort_by_key(|(reset, _, seen)| (*reset, *seen));
    let week = SignedDuration::from_secs(WEEK_SECONDS);
    let mut spans: Vec<Span> = resets
        .into_iter()
        .map(|(reset, account, _)| Span { start: reset - week, end: reset, reset, account })
        .collect();
    spans.push(Span {
        start: current.start,
        end: current.end,
        reset: current.end,
        account: owners.last().map(|(_, key)| key.clone()),
    });
    for index in 1..spans.len() {
        let next = spans[index].start;
        if spans[index - 1].end > next {
            spans[index - 1].end = next;
        }
    }
    spans.retain(|span| span.start < span.end);
    spans
}

/// What a week's committed per-model uses went to.
#[derive(Default)]
struct Tally {
    models: BTreeMap<String, UsageSum>,
    tokens: Tokens,
    kinds: Option<KindsUsd>,
    legacy: BTreeMap<String, (Tokens, Option<Usd>)>,
}
impl Tally {
    fn add(&mut self, usage: &Use) {
        self.models.entry(usage.model.clone()).or_default().add(usage.tokens.total(), usage.usd);
        self.tokens.add(&usage.tokens);
        if let Some(kinds) = usage.kinds {
            super::claude::add_kinds(self.kinds.get_or_insert_with(Default::default), kinds);
        } else {
            // Old rows kept costs and turn tokens separately. Pair only those
            // legacy quantities by model, without reclassifying modern requests.
            let (tokens, usd) = self.legacy.entry(usage.model.clone()).or_default();
            tokens.add(&usage.tokens);
            if let Some(amount) = usage.usd {
                *usd = Some(usd.unwrap_or_default().saturating_add(amount));
            }
        }
    }
    fn went(mut self) -> WeekUse {
        for (model, (tokens, usd)) in &self.legacy {
            if let Some(usd) = usd {
                super::claude::add_kinds(
                    self.kinds.get_or_insert_with(Default::default),
                    super::claude::kinds(model, tokens, *usd),
                );
            }
        }
        let mut models: Vec<ModelUse> = self.models.into_iter().map(|(model, sum)| ModelUse { model, sum }).collect();
        models.sort_by(|a, b| {
            b.sum.usd.cmp(&a.sum.usd).then(b.sum.tokens.cmp(&a.sum.tokens)).then(a.model.cmp(&b.model))
        });
        WeekUse { models, tokens: self.tokens, kinds_usd: self.kinds }
    }
}

pub(super) fn boundaries(spans: &[Span], week: Week) -> Vec<Timestamp> {
    let mut cuts: Vec<_> = spans.iter().flat_map(|span| [span.start, span.end]).collect();
    if week.window {
        cuts.extend([week.start, week.end]);
    }
    cuts.sort();
    cuts.dedup();
    cuts
}

/// Hour buckets are split at every recorded weekly boundary.
pub(super) fn bucket(at: Timestamp, boundaries: &[Timestamp]) -> Timestamp {
    let hour = Timestamp::from_second(at.as_second().div_euclid(3600) * 3600).expect("existing hour");
    // A known reset establishes the cut even in weeks with no sample. It does
    // not create a historical week in the report's list of observed periods.
    boundaries
        .iter()
        .map(|cut| {
            let weeks = at.duration_since(*cut).as_secs().div_euclid(WEEK_SECONDS);
            *cut + SignedDuration::from_secs(weeks * WEEK_SECONDS)
        })
        .filter(|cut| *cut <= at && *cut > hour)
        .max()
        .unwrap_or(hour)
}
