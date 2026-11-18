//! Assistance limits use the committed eight-day spending index, without rereading files.
use super::reckoning::quota;
use crate::Node;
use fastctx_net::directory::Assist;
use fastctx_types::access::{Acting, HubAccount};
use fastctx_types::residents::About;
use fastctx_types::usage::Usd;
use jiff::{SignedDuration, Timestamp};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

const DAY: SignedDuration = SignedDuration::from_hours(24);
const WEEK: SignedDuration = SignedDuration::from_hours(7 * 24);
/// A login's week where its owner set none, in quota dollars (用户
/// 2026-11-15: 默认3000刀).
pub(crate) const DEFAULT_WEEK: Usd = Usd(3_000_000_000);
/// The part of a login's week Fable may use, all who use it together, the
/// owner too, in percent (用户 2026-11-15: 线，我们定50%): Fable's own
/// allowance in the vendor's accounting, which no reading tells.
const FABLE_LINE: u64 = 50;

#[derive(Clone)]
pub(crate) enum UsedUp {
    Weekly { allowance: Usd, until: Timestamp },
    Daily { allowance: Usd, until: Timestamp },
    FiveHour { share: u8, until: Option<Timestamp> },
    Unavailable,
    Fable { allowance: Usd, until: Timestamp },
    /// The whole login's Fable reached its line: everyone's Fable waits,
    /// the owner's too.
    FableLine { allowance: Usd, until: Timestamp },
}
impl UsedUp {
    pub(super) fn until(&self) -> Timestamp {
        match self {
            Self::Weekly { until, .. } | Self::Daily { until, .. } | Self::Fable { until, .. } | Self::FableLine { until, .. } => *until,
            Self::FiveHour { until, .. } => until.unwrap_or(Timestamp::MAX),
            Self::Unavailable => Timestamp::MAX,
        }
    }
    pub(crate) fn expires(&self) -> Option<Timestamp> {
        (self.until() != Timestamp::MAX).then(|| self.until())
    }
    pub(super) fn fable(&self) -> bool {
        matches!(self, Self::Fable { .. } | Self::FableLine { .. })
    }
    pub(crate) fn applies(&self, model: Option<&str>) -> bool {
        !self.fable() || model.is_some_and(fastctx_hosts::claude::pricing::is_fable)
    }
    /// What the hold says, to the account it holds, or to everyone who uses
    /// the login where it holds them all.
    pub(crate) fn said(&self, account: Option<&HubAccount>) -> String {
        let dollars = |usd: &Usd| usd.0 as f64 / 1_000_000.0;
        if let Self::FableLine { allowance, until } = self {
            return format!(
                "this login's Fable has used its line, half the login's week (${:.2}), shared by everyone who uses it; Fable waits until {}, its other models go on",
                dollars(allowance),
                until.strftime("%Y-%m-%d %H:%M UTC")
            );
        }
        let Some(account) = account else {
            return "this login's usage holds this conversation; what is for it waits".into();
        };
        if matches!(self, Self::Unavailable) {
            return "this login's usage ledger cannot be checked; assistance is paused until accounting recovers"
                .into();
        }
        if let Self::FiveHour { share, until: None } = self {
            return format!(
                "this login's five-hour window is past the {share}% assistance limit; assistance waits for a new reading because its reset time is unknown"
            );
        }
        if let Self::Fable { allowance, until } = self {
            return format!(
                "the Fable allowance of ${:.2} this login gives {} is used up until {}; its other models go on",
                dollars(allowance),
                account.account,
                until.strftime("%Y-%m-%d %H:%M UTC")
            );
        }
        let what = match self {
            Self::Weekly { allowance, .. } => format!(
                "the weekly allowance of ${:.2} this login gives {} is used up",
                dollars(allowance),
                account.account
            ),
            Self::Daily { allowance, .. } => format!(
                "the daily allowance of ${:.2} this login gives {} is used up",
                dollars(allowance),
                account.account
            ),
            Self::FiveHour { share, .. } => {
                format!("this login's five-hour window is past the {share}% it assists {} up to", account.account)
            }
            Self::Fable { .. } | Self::FableLine { .. } | Self::Unavailable => unreachable!(),
        };
        format!("{what} until {}; what is for it waits until then", self.until().strftime("%Y-%m-%d %H:%M UTC"))
    }
}

fn assistance(node: &Node, about: &About, account: &HubAccount) -> Result<Option<Assist>, fastctx_types::ipc::Failure> {
    let Some(link) = node.link_of(&account.hub) else { return Ok(None) };
    let view = link.view();
    let Some(here) = view.me().map(|member| member.name.clone()) else { return Ok(None) };
    if view.logged_in(&here, &account.account) || view.rights(&account.account, &here).hands {
        return Ok(None);
    }
    crate::homes::assistance(node, &view, &account.account, &here, &about.host.agent, about.home.as_deref())
}

pub(super) fn login(node: &Node, about: &About) -> Option<PathBuf> {
    about.home.clone().or_else(|| match about.host.agent.as_str() {
        "claude" => super::claude_default(),
        "codex" => Some(node.codex.default().config_home().to_path_buf()),
        _ => None,
    })
}

/// With a model, considers only restrictions on that next request. Without one,
/// prefers an all-model hold; its expiry will expose any remaining Fable hold.
/// The whole login's Fable line holds every account, its owner's too.
pub(crate) fn used_up(node: &Node, about: &About, account: &HubAccount, model: Option<&str>) -> Option<UsedUp> {
    let mut reached: Vec<UsedUp> = assisted(node, about, account, model).into_iter().collect();
    reached.extend(fable_line(node, about));
    choose(reached, model)
}

/// The hold that matters of those reached: with a model, those on it; without
/// one, an all-model hold before a Fable one.
fn choose(mut reached: Vec<UsedUp>, model: Option<&str>) -> Option<UsedUp> {
    if model.is_some() {
        reached.retain(|used| used.applies(model));
    } else if reached.iter().any(|used| !used.fable()) {
        reached.retain(|used| !used.fable());
    }
    reached.into_iter().max_by_key(UsedUp::until)
}

/// Whether the whole Claude Code login a conversation is in has used its Fable
/// line this week: the Fable all who use it spent, in quota dollars, as it
/// counts after the resets made by hand, against half the week's figure.
pub(crate) fn fable_line(node: &Node, about: &About) -> Option<UsedUp> {
    if about.host.agent.as_str() != "claude" {
        return None;
    }
    let dir = login(node, about)?;
    let path = super::ledger(node, "claude", &dir);
    let now = Timestamp::now();
    super::with_book(node, &path, |book| {
        let reckoned = super::reckoning::of(&path, book, "claude", now);
        let (week, allowance) = (reckoned.week, Usd(reckoned.figure.0 / 100 * FABLE_LINE));
        let fable: Vec<_> = book.recent.iter().filter(|charge| charge.fable && charge.at <= now).map(|charge| (charge.at, quota(charge))).collect();
        let until = if week.window {
            let spent = fable
                .iter()
                .filter(|(at, _)| *at >= week.start)
                .fold(Usd::ZERO, |sum, (at, usd)| sum.saturating_add(reckoned.counted(*at, *usd)));
            (spent >= allowance).then_some(week.end)
        } else {
            rolling(&fable, allowance, WEEK, now)
        };
        Ok(until.map(|until| UsedUp::FableLine { allowance, until }))
    })
    .unwrap_or_else(|error| {
        node.log.line(format_args!("reading the Fable a Claude login used: {error}"));
        None
    })
}

/// What the account's assistance in the login holds it to.
fn assisted(node: &Node, about: &About, account: &HubAccount, model: Option<&str>) -> Option<UsedUp> {
    let assist = match assistance(node, about, account) {
        Ok(assist) => assist?,
        Err(error) => {
            node.log.line(format_args!("identifying the conversation's assistance: {error}"));
            return Some(UsedUp::Unavailable);
        }
    };
    let agent = about.host.agent.as_str();
    if assist.five_hour.is_none() && (agent != "claude" || (assist.weekly.is_none() && assist.daily.is_none() && assist.share.is_none())) {
        return None;
    }
    let dir = login(node, about)?;
    let path = super::ledger(node, agent, &dir);
    let now = Timestamp::now();
    let reached = super::with_book(node, &path, |book| {
        let mut reached = Vec::new();
        // Every five-hour allowance still in its period is held to the line,
        // as Codex's several buckets are; one past its reset says nothing of
        // the others. Assistance waits until the last over the line resets.
        if let Some(share) = assist.five_hour
            && let Some((_, windows)) = &book.reading
        {
            let over: Vec<_> = windows
                .iter()
                .filter(|window| window.minutes == Some(300))
                .filter(|window| window.resets_at.is_none_or(|until| until > now))
                .filter(|window| window.used_tenths >= u32::from(share) * 10)
                .collect();
            if !over.is_empty() {
                let until = if over.iter().any(|window| window.resets_at.is_none()) {
                    None
                } else {
                    over.iter().filter_map(|window| window.resets_at).max()
                };
                reached.push(UsedUp::FiveHour { share, until });
            }
        }
        if agent != "claude" {
            return Ok(reached);
        }
        // A share is of the week's figure, in quota dollars; what was used
        // before a reset made by hand counts as the part the reset left.
        let reckoned = super::reckoning::of(&path, book, agent, now);
        let week = reckoned.week;
        let share = assist.share.map(|share| Usd(reckoned.figure.0 / 100 * u64::from(share)));
        let balances: Vec<_> = book
            .balances()
            .into_iter()
            .flat_map(|row| {
                let at = row.at();
                row.uses().into_iter().filter_map(move |(who, usage)| {
                    Some(super::recent::Charge { at, who, fable: false, usd: usage.usd? })
                })
            })
            .collect();
        let mut costs: Vec<_> = book
            .recent
            .iter()
            .chain(&balances)
            .filter(|charge| charge.who.as_ref() == Some(account) && charge.at <= now)
            .collect();
        costs.sort_by_key(|charge| charge.at);
        let all: Vec<_> = costs.iter().map(|charge| (charge.at, charge.usd)).collect();
        let weekly = |costs: &[(Timestamp, Usd)], cap| {
            if week.window {
                let spent = costs
                    .iter()
                    .filter(|(at, _)| *at >= week.start)
                    .fold(Usd::ZERO, |sum, (at, usd)| sum.saturating_add(reckoned.counted(*at, *usd)));
                (spent >= cap).then_some(week.end)
            } else {
                rolling(costs, cap, WEEK, now)
            }
        };
        if let Some(allowance) = share {
            // Held in quota dollars: what each request used of the quota.
            let used: Vec<_> = costs.iter().map(|charge| (charge.at, quota(charge))).collect();
            if let Some(until) = weekly(&used, allowance) {
                reached.push(UsedUp::Weekly { allowance, until });
            }
            let fable: Vec<_> = costs.iter().filter(|charge| charge.fable).map(|charge| (charge.at, quota(charge))).collect();
            let allowance = Usd(allowance.0 / 2);
            if let Some(until) = weekly(&fable, allowance) {
                reached.push(UsedUp::Fable { allowance, until });
            }
        } else if let Some(allowance) = assist.weekly {
            if let Some(until) = weekly(&all, allowance) {
                reached.push(UsedUp::Weekly { allowance, until });
            }
            let fable: Vec<_> =
                costs.iter().filter(|charge| charge.fable).map(|charge| (charge.at, charge.usd)).collect();
            let allowance = Usd(allowance.0 / 2);
            if let Some(until) = weekly(&fable, allowance) {
                reached.push(UsedUp::Fable { allowance, until });
            }
        }
        if let Some(allowance) = assist.daily
            && let Some(until) = rolling(&all, allowance, DAY, now)
        {
            reached.push(UsedUp::Daily { allowance, until });
        }
        Ok(reached)
    })
    .unwrap_or_else(|error| {
        node.log.line(format_args!("reading what {} spent of a Claude login: {error}", account.account));
        vec![UsedUp::Unavailable]
    });
    choose(reached, model)
}

fn rolling(costs: &[(Timestamp, Usd)], allowance: Usd, span: SignedDuration, now: Timestamp) -> Option<Timestamp> {
    let within: Vec<_> = costs.iter().filter(|(at, _)| *at > now - span).collect();
    let mut spent = within.iter().fold(Usd::ZERO, |sum, (_, usd)| sum.saturating_add(*usd));
    if spent < allowance {
        return None;
    }
    for (at, usd) in within {
        spent = spent.saturating_sub(*usd);
        if spent < allowance {
            return Some(*at + span);
        }
    }
    Some(now + span)
}

/// All conversations are checked, including idle roots with busy background
/// agents: heavy work, done on a thread that may block.
pub(crate) async fn after_reading(node: &Arc<Node>, agent: &str, dir: &std::path::Path) {
    // What the login's row in the list of logins says of its usage is read
    // anew behind this.
    crate::logins::usage_read(node, agent, dir);
    let (node, agent, dir) = (node.clone(), agent.to_string(), dir.to_path_buf());
    fastctx_platform::task::blocking(move || super::hold::refresh(&node, Some((&agent, &dir)))).await;
}

/// What the accounts a login's conversations act for have used up, each
/// account read once for all of them: what an account has used up depends
/// only on it and the login, and a login may hold hundreds of conversations.
/// The machine's own conversations are held by the login's Fable line alone.
#[derive(Default)]
pub(super) struct Restrictions(HashMap<(Option<HubAccount>, Option<PathBuf>), Option<UsedUp>>);

impl Restrictions {
    /// The restriction on `about`, a conversation of the login these are for,
    /// with the account it holds; none where it holds the machine's own.
    pub(super) fn of(&mut self, node: &Node, about: &About) -> Option<(Option<HubAccount>, UsedUp)> {
        let Acting::Accounts { accounts } = &about.acting else {
            let line = self.0.entry((None, about.home.clone())).or_insert_with(|| fable_line(node, about));
            return line.clone().map(|used| (None, used));
        };
        accounts
            .iter()
            .filter_map(|account| {
                let used = self
                    .0
                    .entry((Some(account.clone()), about.home.clone()))
                    .or_insert_with(|| used_up(node, about, account, None));
                used.clone().map(|used| (Some(account.clone()), used))
            })
            .max_by_key(|(_, used)| (!used.fable(), used.until()))
    }
}
