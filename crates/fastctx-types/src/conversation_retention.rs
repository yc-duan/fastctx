//! Deleting a conversation, with all it keeps, once it has been idle a
//! number of days. The rules are kept on the machine the conversation's agent
//! lives on, one for each network it is on: there its owner sets the days for
//! their own conversations and, as a ceiling, for those of the accounts its
//! agents assist; each assisted account may set fewer days for its own. Off
//! until set; turned on or shortened, nothing it touches goes for
//! `GRACE_DAYS`.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::ResidentId;
use crate::access::HubAccount;

/// The days a rule may be set to.
pub const DAYS: [u32; 6] = [3, 7, 14, 30, 60, 90];
/// How long after a rule is turned on or shortened before anything goes.
pub const GRACE_DAYS: i64 = 7;

/// A machine's rule on one network.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rule {
    /// For the owner's own conversations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub own_days: Option<u32>,
    /// For the conversations of the accounts the machine's agents assist, at
    /// most.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assisted_days: Option<u32>,
    /// What each assisted account set for its own conversations.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub accounts: Vec<AccountDays>,
    /// Nothing goes before this, the owner having turned the rule on or
    /// shortened it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grace_until: Option<Timestamp>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountDays {
    pub of: HubAccount,
    pub days: u32,
    /// None of its conversations goes before this, the account having
    /// shortened its own days.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grace_until: Option<Timestamp>,
}

impl Rule {
    /// Whether anything is ever deleted by it.
    pub fn on(&self) -> bool {
        self.own_days.is_some() || self.assisted_days.is_some() || !self.accounts.is_empty()
    }

    /// The days a conversation of an assisted account `of` is kept idle.
    pub fn assisted(&self, of: &HubAccount) -> Option<u32> {
        let own = self.accounts.iter().find(|set| set.of == *of).map(|set| set.days);
        match (self.assisted_days, own) {
            (Some(ceiling), Some(own)) => Some(ceiling.min(own)),
            (ceiling, own) => ceiling.or(own),
        }
    }

    /// Before when none of the conversations of an assisted account `of`
    /// goes: the owner's grace, or the account's own, whichever is later.
    pub fn assisted_grace(&self, of: &HubAccount) -> Option<Timestamp> {
        let own = self.accounts.iter().find(|set| set.of == *of).and_then(|set| set.grace_until);
        self.grace_until.max(own)
    }
}

/// Why `days` cannot be set, where it cannot.
pub fn problem(days: Option<u32>) -> Option<String> {
    days.filter(|days| !DAYS.contains(days))
        .map(|days| format!("a conversation is kept idle one of {DAYS:?} days, not {days}"))
}

/// What is asked of a machine's rule.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "do", rename_all = "snake_case")]
pub enum RetentionAsk {
    Read,
    /// The owner's two settings; `None` turns one off.
    Owner {
        own_days: Option<u32>,
        assisted_days: Option<u32>,
    },
    /// An assisted account's own, fewer days; `None` follows the owner's.
    Mine {
        days: Option<u32>,
    },
}

/// A machine's rule on the asker's network as the asker may know it, and
/// when each of the conversations the asker may see goes.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetentionAnswer {
    pub rule: Rule,
    /// Whether the asker is the machine's owner.
    pub owner: bool,
    pub conversations: Vec<Due>,
}

/// Since when a conversation has been idle, and when it goes as the rule
/// stands: never, where the rule keeps it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Due {
    pub resident: ResidentId,
    /// Since when it has been idle: its last words, turn or command.
    pub idle_since: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub days: Option<u32>,
    /// When it goes if nothing happens in it before: at least after the
    /// grace of a rule turned on or shortened.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<Timestamp>,
}
