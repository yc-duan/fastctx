//! The weekly allowance an assistance gives (`world-network.md`, 共享机器与智能体协助;
//! `world-agent-logins.md`, 用量): what an account a Claude Code login
//! assists may spend of it in a week of the login's own seven-day window,
//! by Claude Code's own cost ledger. A conversation working for it takes no
//! new turn once that is spent, and the turn running when it is reached
//! stops there; with the next week it goes on.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use fastctx_types::ResidentId;
use fastctx_types::access::{Acting, HubAccount};
use fastctx_types::residents::About;
use fastctx_types::session::{Health, HealthKind};
use fastctx_types::usage::Usd;
use jiff::Timestamp;

use super::Entry;
use crate::Node;

/// An allowance used up: how much it was, and when its week ends.
pub(crate) struct UsedUp {
    pub weekly: Usd,
    pub until: Timestamp,
}

impl UsedUp {
    /// What a conversation working for `account` is told, and the user with
    /// it: the words the interface recognizes it by begin it.
    pub(crate) fn said(&self, account: &HubAccount) -> String {
        format!(
            "the weekly allowance of ${:.2} this login gives {} is used up until {}; what is for it waits until then",
            self.weekly.0 as f64 / 1_000_000.0,
            account.account,
            self.until.strftime("%Y-%m-%d %H:%M UTC")
        )
    }
}

/// The allowance `account` has on the Claude Code login `about` runs in
/// here: only an assistance gives one; none where the account may use the
/// login otherwise, being logged in here or given the machine.
fn allowance(node: &Node, about: &About, account: &HubAccount) -> Option<Usd> {
    let link = node.link_of(&account.hub)?;
    let view = link.view();
    let here = view.me()?.name.clone();
    if view.logged_in(&here, &account.account) || view.rights(&account.account, &here).hands {
        return None;
    }
    view.assisting(&account.account, &here)
        .find(|assist| assist.is(&about.host.agent, about.home.as_deref()))
        .and_then(|assist| assist.weekly)
}

/// The login a conversation the World started runs in.
fn login(about: &About) -> Option<PathBuf> {
    about.home.clone().or_else(super::claude_default)
}

/// Whether `account`'s allowance on the login `about` runs in is used up
/// in the login's current week. Only Claude Code keeps a cost ledger.
pub(crate) fn used_up(node: &Node, about: &About, account: &HubAccount) -> Option<UsedUp> {
    if about.host.agent.as_str() != "claude" {
        return None;
    }
    let weekly = allowance(node, about, account)?;
    let ledger = super::ledger(node, "claude", &login(about)?);
    let week = super::week(node, &ledger, Timestamp::now());
    match spent(&ledger, account, week.start, week.end) {
        Ok(spent) => (spent >= weekly).then_some(UsedUp { weekly, until: week.end }),
        Err(error) => {
            node.log.line(format_args!("reading what {} spent of a Claude login: {error}", account.account));
            None
        }
    }
}

/// What `account` spent of the login whose ledger is `ledger` in the week
/// from `start` to `end`.
pub(super) fn spent(ledger: &Path, account: &HubAccount, start: Timestamp, end: Timestamp) -> std::io::Result<Usd> {
    Ok(super::entries(ledger, start)?
        .into_iter()
        .filter_map(|entry| match entry {
            Entry::Cost { at, who: Some(who), usd, .. } if who == *account && at < end => Some(usd),
            _ => None,
        })
        .fold(Usd::ZERO, Usd::saturating_add))
}

/// After a rise of the cost of `id`'s session: when it works for an account
/// whose allowance that used up, its running turn stops, and the record
/// says why.
pub(crate) async fn after_cost(node: &Arc<Node>, id: &ResidentId, about: &About) {
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
        node.log.line(format_args!("stopping {id} at its weekly allowance: {}", failure.message));
    }
    let health = Health { kind: HealthKind::Warning, text: used.said(account) };
    if let Err(error) = crate::turns::health(node, id, None, health) {
        node.log.line(format_args!("recording that {id} reached its weekly allowance: {error}"));
    }
}
