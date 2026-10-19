//! The allowances an assistance gives (`world-network.md`, 共享机器与智能体协助;
//! `world-agent-logins.md`, 用量). An account a Claude Code login assists
//! may spend of it what the assistance allows in a week of the login's own
//! seven-day window and in any 24 hours, by Claude Code's own cost ledger.
//! A login of either agent may also assist only while its own five-hour
//! window is under a share, its owner's use counted with everyone's. A
//! conversation working for the account takes no new turn once one of them
//! is reached, and the turn running when it is reached stops there; it goes
//! on once the spending falls back under it or the window begins again.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use fastctx_net::directory::Assist;
use fastctx_types::ResidentId;
use fastctx_types::access::{Acting, HubAccount};
use fastctx_types::residents::About;
use fastctx_types::session::{Health, HealthKind};
use fastctx_types::usage::Usd;
use jiff::{SignedDuration, Timestamp};

use super::Entry;
use crate::Node;

const DAY: SignedDuration = SignedDuration::from_hours(24);
const FIVE_HOURS: SignedDuration = SignedDuration::from_hours(5);

/// An allowance reached, and when it no longer is.
pub(crate) enum UsedUp {
    /// The weekly allowance is spent until its week ends.
    Weekly { allowance: Usd, until: Timestamp },
    /// The daily allowance is spent until enough of what was spent is a
    /// day old.
    Daily { allowance: Usd, until: Timestamp },
    /// The login's five-hour window is at the share it assists up to until
    /// the window begins again.
    FiveHour { share: u8, until: Timestamp },
}

impl UsedUp {
    fn until(&self) -> Timestamp {
        match self {
            Self::Weekly { until, .. } | Self::Daily { until, .. } | Self::FiveHour { until, .. } => *until,
        }
    }

    /// What a conversation working for `account` is told, and the user with
    /// it: the words the interface recognizes it by begin it.
    pub(crate) fn said(&self, account: &HubAccount) -> String {
        let dollars = |usd: &Usd| usd.0 as f64 / 1_000_000.0;
        let what = match self {
            Self::Weekly { allowance, .. } => {
                format!("the weekly allowance of ${:.2} this login gives {} is used up", dollars(allowance), account.account)
            }
            Self::Daily { allowance, .. } => {
                format!("the daily allowance of ${:.2} this login gives {} is used up", dollars(allowance), account.account)
            }
            Self::FiveHour { share, .. } => {
                format!("this login's five-hour window is past the {share}% it assists {} up to", account.account)
            }
        };
        format!(
            "{what} until {}; what is for it waits until then",
            self.until().strftime("%Y-%m-%d %H:%M UTC")
        )
    }
}

/// The assistance by which `account` uses the login `about` runs in here:
/// none where the account may use the login otherwise, being logged in
/// here or given the machine, and nothing then holds it back.
fn assistance(node: &Node, about: &About, account: &HubAccount) -> Option<Assist> {
    let link = node.link_of(&account.hub)?;
    let view = link.view();
    let here = view.me()?.name.clone();
    if view.logged_in(&here, &account.account) || view.rights(&account.account, &here).hands {
        return None;
    }
    view.assisting(&account.account, &here).find(|assist| assist.is(&about.host.agent, about.home.as_deref())).cloned()
}

/// The login a conversation the World started runs in.
fn login(node: &Node, about: &About) -> Option<PathBuf> {
    if let Some(home) = &about.home {
        return Some(home.clone());
    }
    match about.host.agent.as_str() {
        "claude" => super::claude_default(),
        "codex" => Some(node.codex.default().config_home().to_path_buf()),
        _ => None,
    }
}

/// Which of `account`'s allowances on the login `about` runs in is reached
/// now, the one that lasts longest where several are. Only Claude Code
/// keeps a cost ledger; the five-hour window is read for either agent.
pub(crate) fn used_up(node: &Node, about: &About, account: &HubAccount) -> Option<UsedUp> {
    let assist = assistance(node, about, account)?;
    let agent = about.host.agent.as_str();
    let ledger = super::ledger(node, agent, &login(node, about)?);
    let now = Timestamp::now();
    let mut reached = Vec::new();
    if let Some(share) = assist.five_hour {
        reached.extend(five_hour(node, &ledger, share, now));
    }
    if agent == "claude" && (assist.weekly.is_some() || assist.daily.is_some()) {
        match spending(node, &ledger, account, &assist, now) {
            Ok(found) => reached.extend(found),
            Err(error) => {
                node.log.line(format_args!("reading what {} spent of a Claude login: {error}", account.account))
            }
        }
    }
    reached.into_iter().max_by_key(UsedUp::until)
}

/// Whether the login's five-hour window, as its last reading had it, is at
/// `share` percent and has not begun again since.
fn five_hour(node: &Node, ledger: &Path, share: u8, now: Timestamp) -> Option<UsedUp> {
    let (read, windows) = super::last_reading(node, ledger)?;
    let window = windows.into_iter().find(|window| window.minutes == Some(300))?;
    let until = window.resets_at.unwrap_or(read + FIVE_HOURS);
    (until > now && window.used_tenths >= u32::from(share) * 10).then_some(UsedUp::FiveHour { share, until })
}

/// The allowances on what `account` spent that are reached: the weekly one
/// in the login's week, the daily one in the last 24 hours.
fn spending(
    node: &Node,
    ledger: &Path,
    account: &HubAccount,
    assist: &Assist,
    now: Timestamp,
) -> std::io::Result<Vec<UsedUp>> {
    let week = super::week(node, ledger, now);
    let mut costs: Vec<(Timestamp, Usd)> = super::entries(ledger, week.start.min(now - DAY))?
        .into_iter()
        .filter_map(|entry| match entry {
            Entry::Cost { at, who: Some(who), usd, .. } if who == *account && at <= now => Some((at, usd)),
            _ => None,
        })
        .collect();
    costs.sort_by_key(|(at, _)| *at);
    let mut reached = Vec::new();
    if let Some(allowance) = assist.weekly {
        let until = if week.window {
            let spent = costs
                .iter()
                .filter(|(at, _)| *at >= week.start)
                .fold(Usd::ZERO, |spent, (_, usd)| spent.saturating_add(*usd));
            (spent >= allowance).then_some(week.end)
        } else {
            rolling(&costs, allowance, SignedDuration::from_hours(7 * 24), now)
        };
        reached.extend(until.map(|until| UsedUp::Weekly { allowance, until }));
    }
    if let Some(allowance) = assist.daily {
        reached.extend(rolling(&costs, allowance, DAY, now).map(|until| UsedUp::Daily { allowance, until }));
    }
    Ok(reached)
}

/// When what was spent in the `span` up to now, of `costs` oldest first,
/// falls back under `allowance` as its oldest part grows older than the
/// span; none when it is under it now.
fn rolling(costs: &[(Timestamp, Usd)], allowance: Usd, span: SignedDuration, now: Timestamp) -> Option<Timestamp> {
    let within: Vec<_> = costs.iter().filter(|(at, _)| *at >= now - span).collect();
    let mut spent = within.iter().fold(Usd::ZERO, |spent, (_, usd)| spent.saturating_add(*usd));
    if spent < allowance {
        return None;
    }
    for (at, usd) in &within {
        spent = spent.saturating_sub(*usd);
        if spent < allowance {
            return Some(*at + span);
        }
    }
    Some(now + span)
}

/// After a change in what `id`'s session spent or read of its login: when
/// it works for an account whose allowance that reached, its running turn
/// stops, and the record says why.
pub(crate) async fn stop_at_allowance(node: &Arc<Node>, id: &ResidentId, about: &About) {
    let Acting::Accounts { accounts } = &about.acting else { return };
    let Some((account, used)) =
        accounts.iter().find_map(|account| used_up(node, about, account).map(|used| (account, used)))
    else {
        return;
    };
    if node.residents.now(id).is_none_or(|now| now.turn.is_none()) {
        return;
    }
    if let Err(failure) = crate::talk::stop_here(node, id).await {
        node.log.line(format_args!("stopping {id} at its allowance: {}", failure.message));
    }
    let health = Health { kind: HealthKind::Warning, text: used.said(account) };
    if let Err(error) = crate::turns::health(node, id, None, health) {
        node.log.line(format_args!("recording that {id} reached its allowance: {error}"));
    }
}

/// After a change of the windows of `agent`'s login `dir`: each turn
/// running on it for an account whose allowance that reached stops, a
/// reading the owner's own use brought counting as much as any other.
pub(crate) async fn after_reading(node: &Arc<Node>, agent: &str, dir: &Path) {
    let running: Vec<(ResidentId, About)> = node
        .residents
        .everyone()
        .filter(|record| {
            record.removed.is_none()
                && record.about.host.agent.as_str() == agent
                && matches!(record.about.acting, Acting::Accounts { .. })
                && record.now.as_ref().is_some_and(|now| now.turn.is_some())
        })
        .filter(|record| login(node, &record.about).is_some_and(|login| crate::homes::same_dir(&login, dir)))
        .map(|record| (record.id, record.about))
        .collect();
    for (id, about) in running {
        stop_at_allowance(node, &id, &about).await;
    }
}
