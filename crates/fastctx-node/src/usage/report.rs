//! A login's use as one reader may see it: everyone's to an account that may
//! work on this machine, its own alone to an account the login assists.

use std::collections::BTreeMap;
use std::io;
use std::path::Path;

use fastctx_types::access::HubAccount;
use fastctx_types::usage::{
    KindsUsd, ModelUse, Tokens, UsageHour, UsagePerson, UsageReading, UsageReport, UsageSum, UsageWeek, Usd,
};
use jiff::Timestamp;

use super::Entry;
use crate::Node;

/// What a token of each kind costs, against one of uncached input, at
/// Anthropic's list prices for every model: a cache read a tenth, a cache
/// write a quarter more, output five times. They split what the ledger
/// says a week cost by kind; the ledger itself says what it cost in all.
const READ: f64 = 0.1;
const WRITE: f64 = 1.25;
const OUTPUT: f64 = 5.0;

/// `agent`'s login `dir` here, as `only` may see it: everyone's use, or the
/// use of that one account alone.
pub(crate) fn report(node: &Node, agent: &str, dir: &Path, only: Option<&HubAccount>) -> io::Result<UsageReport> {
    let ledger = super::ledger(node, agent, dir);
    let now = Timestamp::now();
    let since = now - jiff::SignedDuration::from_hours(24 * super::KEPT_DAYS);
    let week = super::week(node, &ledger, now);
    let in_week = |at: Timestamp| at >= week.start && at < week.end.max(now);
    let mut people: Vec<Option<HubAccount>> = Vec::new();
    let mut person = |who: Option<HubAccount>| -> u32 {
        let index = people.iter().position(|known| *known == who).unwrap_or_else(|| {
            people.push(who);
            people.len() - 1
        });
        u32::try_from(index).expect("fewer people than u32 counts")
    };
    let mut hours: BTreeMap<(i64, u32), UsageSum> = BTreeMap::new();
    let mut weekly: BTreeMap<u32, UsageSum> = BTreeMap::new();
    let mut models: BTreeMap<String, UsageSum> = BTreeMap::new();
    let mut tokens = Tokens::default();
    let mut spent: Option<Usd> = None;
    let mut readings = Vec::new();
    for entry in super::entries(&ledger, since)? {
        match entry {
            Entry::Cost { at, who, model, usd } => {
                if only.is_some_and(|only| who.as_ref() != Some(only)) {
                    continue;
                }
                let index = person(who);
                hours.entry((hour(at), index)).or_default().add(0, Some(usd));
                if in_week(at) {
                    weekly.entry(index).or_default().add(0, Some(usd));
                    models.entry(model).or_default().add(0, Some(usd));
                    spent = Some(spent.unwrap_or(Usd::ZERO).saturating_add(usd));
                }
            }
            Entry::Tokens { at, who, model, tokens: used } => {
                if only.is_some_and(|only| who.as_ref() != Some(only)) {
                    continue;
                }
                let index = person(who);
                hours.entry((hour(at), index)).or_default().add(used.total(), None);
                if in_week(at) {
                    weekly.entry(index).or_default().add(used.total(), None);
                    models.entry(model).or_default().add(used.total(), None);
                    tokens.add(&used);
                }
            }
            Entry::Reading { at, windows } => {
                if in_week(at) {
                    readings.push(UsageReading { at, windows });
                }
            }
        }
    }
    let mut models: Vec<ModelUse> = models.into_iter().map(|(model, sum)| ModelUse { model, sum }).collect();
    models.sort_by(|a, b| b.sum.usd.cmp(&a.sum.usd).then(b.sum.tokens.cmp(&a.sum.tokens)));
    Ok(UsageReport {
        people: people
            .into_iter()
            .enumerate()
            .map(|(index, account)| UsagePerson {
                account,
                week: weekly.get(&(index as u32)).copied().unwrap_or_default(),
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
        week: UsageWeek {
            start: Some(week.start),
            end: Some(week.end),
            window: week.window,
            models,
            tokens,
            kinds_usd: spent.map(|spent| kinds(&tokens, spent)),
        },
        readings,
    })
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
