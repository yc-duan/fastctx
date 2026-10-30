//! Published HTML and the user's consent in a town's immutable ledger.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::project::{Stamp, Writer};
use crate::{Address, EntryId, JournalName, MachineName, PageName, TownName};

/// One capability, compared literally across declarations and consent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum PageAct {
    Run { machine: MachineName, script: String },
    Say { to: Address },
    Journal { journal: JournalName },
}

/// A complete replacement publication, before the node stamps it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageDraft {
    pub title: String,
    pub description: String,
    pub acts: Vec<PageAct>,
    pub html: String,
}

/// The description file of a publication; its HTML is a separate file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageVersion {
    pub version: u32,
    pub base: Option<u32>,
    pub title: String,
    pub description: String,
    pub acts: Vec<PageAct>,
    pub withdrawn: bool,
    /// Restoring copies that publication into this new version.
    pub restored_from: Option<u32>,
    pub by: Writer,
    /// As a project record's `on`: left out for whoever may not work on
    /// the machine it was published on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on: Option<MachineName>,
    pub at: Timestamp,
}

impl PageVersion {
    /// As told to someone who may work only on the machines `usable`
    /// admits (`Stamp::for_reader`).
    pub fn for_reader(&mut self, usable: &dyn Fn(&MachineName) -> bool) {
        if self.on.as_ref().is_some_and(|on| !usable(on)) {
            self.on = None;
        }
        self.by.for_reader(usable);
    }
}

/// The account that consented, or the user of a project kept only locally.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum PageIdentity {
    Account { hub: crate::access::HubId, account: crate::AccountName },
    Local { machine: MachineName },
}

/// A user's complete consent set, independent of publication versions.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageConsent {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<PageIdentity>,
    pub version: u32,
    pub base: Option<u32>,
    pub page_version: u32,
    pub acts: Vec<PageAct>,
    pub stamp: Stamp,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageSummary {
    pub page: PageName,
    pub current: PageVersion,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageView {
    pub town: TownName,
    pub page: PageName,
    pub publication: PageVersion,
    pub html: Option<String>,
    /// Current consent, including when reading an older publication.
    pub consent: Option<PageConsent>,
}

impl PageView {
    /// As told to someone who may work only on the machines `usable`
    /// admits: who published it and who consented, and where.
    pub fn for_reader(&mut self, usable: &dyn Fn(&MachineName) -> bool) {
        self.publication.for_reader(usable);
        if let Some(consent) = &mut self.consent {
            consent.stamp.for_reader(usable);
        }
    }
}

/// Attribution stamped by the core, never supplied by the embedded HTML.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PageSource {
    /// Missing in earlier records, which cannot authorize new page actions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<PageIdentity>,
    pub town: TownName,
    pub page: PageName,
    pub version: u32,
    pub by: Box<Writer>,
    pub on: MachineName,
    /// The user's device that relayed this action while the page was open.
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

/// The one action requested by an open page. Run has no variable inputs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum PageInvocation {
    Run { machine: MachineName, script: String },
    Say { to: Address, text: String },
    Journal { journal: JournalName, text: String },
}

impl PageInvocation {
    pub fn act(&self) -> PageAct {
        match self {
            Self::Run { machine, script } => PageAct::Run { machine: machine.clone(), script: script.clone() },
            Self::Say { to, .. } => PageAct::Say { to: to.clone() },
            Self::Journal { journal, .. } => PageAct::Journal { journal: journal.clone() },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "do")]
pub enum PageCall {
    Publish {
        town: Option<TownName>,
        page: PageName,
        base: Option<u32>,
        draft: PageDraft,
    },
    Read {
        town: Option<TownName>,
        page: PageName,
        version: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        account: Option<crate::AccountName>,
    },
    List {
        town: Option<TownName>,
    },
    History {
        town: Option<TownName>,
        page: PageName,
    },
    Withdraw {
        town: Option<TownName>,
        page: PageName,
        base: u32,
    },
    Restore {
        town: Option<TownName>,
        page: PageName,
        base: u32,
        version: u32,
    },
    /// Replaces consent; an empty set revokes everything. `base` is the
    /// consent version read by the user, while `version` is the page version.
    Consent {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        account: Option<crate::AccountName>,
        town: Option<TownName>,
        page: PageName,
        version: u32,
        base: Option<u32>,
        acts: Vec<PageAct>,
    },
    Act {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        account: Option<crate::AccountName>,
        town: Option<TownName>,
        page: PageName,
        version: u32,
        action: PageInvocation,
    },
}

impl PageCall {
    pub fn account(&self) -> Option<&crate::AccountName> {
        match self {
            Self::Read { account, .. } | Self::Consent { account, .. } | Self::Act { account, .. } => account.as_ref(),
            _ => None,
        }
    }

    /// A relay uses its authenticated account, never a payload's claim.
    pub fn for_account(&mut self, selected: crate::AccountName) {
        match self {
            Self::Read { account, .. } | Self::Consent { account, .. } | Self::Act { account, .. } => {
                *account = Some(selected)
            }
            _ => {}
        }
    }

    /// The town the call names; `None` means the caller's.
    pub fn town(&self) -> Option<&TownName> {
        match self {
            PageCall::Publish { town, .. }
            | PageCall::Read { town, .. }
            | PageCall::List { town }
            | PageCall::History { town, .. }
            | PageCall::Withdraw { town, .. }
            | PageCall::Restore { town, .. }
            | PageCall::Consent { town, .. }
            | PageCall::Act { town, .. } => town.as_ref(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum PageResult {
    Run(Box<crate::run::RunReply>),
    Said { to: Address, landed: crate::residents::Landed },
    EntryAdded { journal: JournalName, id: EntryId },
}

pub fn version_path(page: &PageName, version: u32) -> String {
    format!("pages/{page}/versions/v{version:06}/page.json")
}

pub fn html_path(page: &PageName, version: u32) -> String {
    format!("pages/{page}/versions/v{version:06}/page.html")
}

pub fn account_consent_path(page: &PageName, identity: &PageIdentity, version: u32) -> String {
    let key = match identity {
        PageIdentity::Account { hub, account } => format!("{hub}-{account}"),
        PageIdentity::Local { .. } => "local".to_string(),
    };
    format!("pages/{page}/consent/{key}/v{version:06}.json")
}

pub fn consent_path(page: &PageName, version: u32) -> String {
    format!("pages/{page}/consent/v{version:06}.json")
}
