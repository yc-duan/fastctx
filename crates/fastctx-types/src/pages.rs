//! What is kept of the pages an earlier release let agents publish as HTML
//! into a project's ledger: removed for good in beta.18. Nothing makes,
//! shows, consents to or runs a page any more; old records still name one —
//! the writer of a ledger entry, the speaker of words a page said, the owner
//! of a job a page ran — and are read as they were written.

use serde::{Deserialize, Serialize};

use crate::project::Writer;
use crate::{MachineName, PageName, TownName};

/// The account that consented, or the user of a project kept only locally.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum PageIdentity {
    Account { hub: crate::access::HubId, account: crate::AccountName },
    Local { machine: MachineName },
}

/// Which page did something, as an old record names it.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PageSource {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<PageIdentity>,
    pub town: TownName,
    pub page: PageName,
    pub version: u32,
    pub by: Box<Writer>,
    pub on: MachineName,
    /// The user's device that relayed the action while the page was open.
    pub opened_on: MachineName,
}

impl std::fmt::Display for PageSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "page {} of {} (version {}, written by ", self.page, self.town, self.version)?;
        match self.by.as_ref() {
            Writer::User => f.write_str("the user")?,
            Writer::Resident { address } => write!(f, "{address}")?,
            Writer::Session { agent } => write!(f, "a {agent} session")?,
            Writer::Page { page } => write!(f, "{page}")?,
            Writer::Elsewhere { page: true } => f.write_str("a page")?,
            Writer::Elsewhere { page: false } => f.write_str("a conversation")?,
        }
        write!(f, " on {}; open on {})", self.on, self.opened_on)
    }
}
