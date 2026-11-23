//! Deleting a conversation, with all it keeps, once it has been idle a
//! number of days. The rule is kept on the machine the conversation's agent
//! lives on: its owner sets the days for their own conversations and, as a
//! ceiling, for those of the accounts its agents assist; each assisted account
//! may set fewer days for its own. Off until set; turned on or shortened,
//! nothing goes for `GRACE_DAYS`.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::ResidentId;
use crate::access::HubAccount;

/// The days a rule may be set to.
pub const DAYS: [u32; 6] = [3, 7, 14, 30, 60, 90];
/// How long after a rule is turned on or shortened before anything goes.
pub const GRACE_DAYS: i64 = 7;

/// A machine's rule.
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
    /// Nothing goes before this, the rule having been turned on or shortened.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grace_until: Option<Timestamp>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountDays {
    pub of: HubAccount,
    pub days: u32,
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

/// A machine's rule as the asker may know it, and when each of the
/// conversations the asker may see goes.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetentionAnswer {
    pub rule: Rule,
    /// Whether the asker is the machine's owner.
    pub owner: bool,
    pub conversations: Vec<Due>,
}

/// When a conversation goes, as the rule stands.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Due {
    pub resident: ResidentId,
    /// Since when it has been idle: its last words, turn or command.
    pub idle_since: Timestamp,
    pub days: u32,
    /// When it goes if nothing happens in it before: at least after the
    /// grace of a rule turned on or shortened.
    pub at: Timestamp,
}
