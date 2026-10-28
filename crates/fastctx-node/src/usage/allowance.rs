//! Assistance limits use the committed eight-day spending index, without rereading files.
use crate::Node;
use fastctx_net::directory::Assist;
use fastctx_types::access::{Acting, HubAccount};
use fastctx_types::residents::About;
use fastctx_types::usage::Usd;
use jiff::{SignedDuration, Timestamp};
use std::path::PathBuf;
use std::sync::Arc;

const DAY: SignedDuration = SignedDuration::from_hours(24);

pub(crate) enum UsedUp {
    Weekly { allowance: Usd, until: Timestamp },
    Daily { allowance: Usd, until: Timestamp },
    FiveHour { share: u8, until: Option<Timestamp> },
    Unavailable,
    Fable { allowance: Usd, until: Timestamp },
}
impl UsedUp {
    pub(super) fn until(&self) -> Timestamp {
        match self {
            Self::Weekly { until, .. } | Self::Daily { until, .. } | Self::Fable { until, .. } => *until,
            Self::FiveHour { until, .. } => until.unwrap_or(Timestamp::MAX),
            Self::Unavailable => Timestamp::MAX,
        }
    }
    pub(crate) fn expires(&self) -> Option<Timestamp> {
        (self.until() != Timestamp::MAX).then(|| self.until())
    }
    pub(super) fn fable(&self) -> bool {
        matches!(self, Self::Fable { .. })
    }
    pub(crate) fn applies(&self, model: Option<&str>) -> bool {
        !self.fable() || model.is_some_and(fastctx_hosts::claude::pricing::is_fable)
    }
    pub(crate) fn said(&self, account: &HubAccount) -> String {
        if matches!(self, Self::Unavailable) {
            return "this login's usage ledger cannot be checked; assistance is paused until accounting recovers"
                .into();
        }
        if let Self::FiveHour { share, until: None } = self {
            return format!(
                "this login's five-hour window is past the {share}% assistance limit; assistance waits for a new reading because its reset time is unknown"
            );
        }
        let dollars = |usd: &Usd| usd.0 as f64 / 1_000_000.0;
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
            Self::Fable { .. } | Self::Unavailable => unreachable!(),
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
pub(crate) fn used_up(node: &Node, about: &About, account: &HubAccount, model: Option<&str>) -> Option<UsedUp> {
    let assist = match assistance(node, about, account) {
        Ok(assist) => assist?,
        Err(error) => {
            node.log.line(format_args!("identifying the conversation's assistance: {error}"));
            return Some(UsedUp::Unavailable);
        }
    };
    let agent = about.host.agent.as_str();
    if assist.five_hour.is_none() && (agent != "claude" || (assist.weekly.is_none() && assist.daily.is_none())) {
        return None;
    }
    let path = super::ledger(node, agent, &login(node, about)?);
    let now = Timestamp::now();
    let mut reached = super::with_book(node, &path, |book| {
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
        let week = super::readings::week(book.reading.as_ref(), now);
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
        if let Some(allowance) = assist.weekly {
            let weekly = |costs: &[(Timestamp, Usd)], cap| {
                if week.window {
                    let spent = costs
                        .iter()
                        .filter(|(at, _)| *at >= week.start)
                        .fold(Usd::ZERO, |sum, (_, usd)| sum.saturating_add(*usd));
                    (spent >= cap).then_some(week.end)
                } else {
                    rolling(costs, cap, SignedDuration::from_hours(7 * 24), now)
                }
            };
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
    if model.is_some() {
        reached.retain(|used| used.applies(model));
    } else if reached.iter().any(|used| !used.fable()) {
        reached.retain(|used| !used.fable());
    }
    reached.into_iter().max_by_key(UsedUp::until)
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

/// All conversations are checked, including idle roots with busy background agents.
pub(crate) async fn after_reading(node: &Arc<Node>, agent: &str, dir: &std::path::Path) {
    super::hold::refresh(node, Some((agent, dir)));
}

pub(super) fn restriction(node: &Node, about: &About) -> Option<(HubAccount, UsedUp)> {
    let Acting::Accounts { accounts } = &about.acting else {
        return None;
    };
    accounts
        .iter()
        .filter_map(|account| used_up(node, about, account, None).map(|used| (account.clone(), used)))
        .max_by_key(|(_, used)| (!used.fable(), used.until()))
}
