//! Goal-review seam. The provider supplies the read-only internal agent at the account's sentinel home.

use super::records::Reply;
use fastctx_types::access::HubAccount;
use fastctx_types::goals::runtime::{ReviewCause, Verdict};
use fastctx_types::turn_evidence::StopReason;
use fastctx_types::{Address, MachineName};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Materials {
    pub account: HubAccount,
    pub conversation: Address,
    pub goal_id: String,
    pub cause: ReviewCause,
    pub suggestions_left: u8,
    /// The owner's words are task data, never reviewer instructions.
    pub objective: String,
    pub plan_claim: Option<String>,
    pub world_actions: Option<Evidence>,
    pub stopped: Option<StopReason>,
    pub last_words_claim: Option<String>,
    pub blocked_claim: Option<String>,
    pub owner_words: Vec<OwnerWords>,
    pub previous_judgments: Vec<String>,
    /// A display catalog; authorization always uses the account's current directory.
    pub machines: Vec<MachineName>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machines_total: Option<usize>,
    pub now: Timestamp,
    /// Complete cause-scoped material on the conversation's machine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_path: Option<String>,
    /// Excerpts or catalog prefixes are present; consult `full_path` for omitted content.
    #[serde(default)]
    pub truncated: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Evidence {
    pub since: Option<Timestamp>,
    pub through: Timestamp,
    /// The immutable local fingerprint snapshot represented by this evidence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<fastctx_types::NoticeId>,
    /// One fact per line, with references to its complete evidence.
    pub summary: String,
    pub full_path: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct OwnerWords {
    pub at: Timestamp,
    pub speaker: fastctx_types::session::Speaker,
    pub text: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "result", content = "value", rename_all = "snake_case")]
pub(crate) enum Answer {
    Judged(Verdict),
    /// No provider attempt has been spent while the sentinel's home is offline.
    Offline {
        reason: String,
    },
    Failed {
        reason: String,
    },
    /// The account has no sentinel to look: the stop stands as the agent
    /// left it, and the user is told.
    Absent {
        reason: String,
    },
}

impl Answer {
    pub(crate) fn validated(self) -> Self {
        match self {
            Self::Judged(Verdict::Failed { reason }) => Self::Failed { reason },
            Self::Judged(Verdict::Sound { ref reason }) if reason.trim().is_empty() => {
                Self::Failed { reason: "The goal reviewer returned no reason.".into() }
            }
            Self::Judged(Verdict::Advice { ref looked, ref saw, ref thinks })
                if [looked, saw, thinks].iter().any(|text| text.trim().is_empty()) =>
            {
                Self::Failed { reason: "The goal reviewer returned an incomplete suggestion.".into() }
            }
            answer => answer,
        }
    }
}

pub(crate) enum Readiness {
    Ready,
    Offline { reason: String },
    /// No sentinel is set up for the account, or its home cannot be used.
    Absent { reason: String },
}

pub(crate) trait Reviewer: Send + Sync {
    /// Checks the configured home's availability without starting a judgment.
    /// Known offline homes wait before material preparation. `review` must still
    /// report Offline if the home disappears after this check.
    fn readiness<'a>(&'a self, account: &'a HubAccount) -> Reply<'a, Readiness>;
    /// `request` is durable and reused after a lost reply. Providers retain the completed judgment.
    fn review<'a>(&'a self, request: &'a str, materials: &'a Materials) -> Reply<'a, Answer>;
}

pub(crate) struct Unconnected;

impl Reviewer for Unconnected {
    fn readiness<'a>(&'a self, _: &'a HubAccount) -> Reply<'a, Readiness> {
        Box::pin(async { Err(fastctx_types::ipc::Failure::new("The goal reviewer is not connected.")) })
    }
    fn review<'a>(&'a self, _: &'a str, _: &'a Materials) -> Reply<'a, Answer> {
        Box::pin(async { Ok(Answer::Failed { reason: "The goal reviewer is not connected.".into() }) })
    }
}
