//! Management work whose catalog edit outlasts a bounded question.

use super::*;
use crate::submitted::Family;
use fastctx_types::{access::TownId, ipc::Failure, submission::Submission};
use serde::{Deserialize, Serialize};
use std::time::Duration;

const FAMILY: Family = Family {
    name: "ask/manage",
    at_once: 4,
    within: Duration::from_secs(10 * 60),
    stays: Duration::from_secs(45),
    // These edits have no comparison or request identity
    // in their original bodies. Repeating can act on later state.
    again: false,
};

/// Only these changes enter a job; password, token and enrollment parameters
/// cannot be represented by this payload.
#[derive(Serialize, Deserialize)]
enum Work {
    DeleteAccount {
        name: AccountName,
        /// As it was opened; none from work kept before this was, which
        /// deletes it by its name alone, as it was asked then.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        opened: Option<jiff::Timestamp>,
    },
    DeleteProject {
        town: TownId,
    },
    RenameGroup {
        owner: AccountName,
        from: String,
        to: Option<String>,
    },
    SetRole {
        name: AccountName,
        visitor: bool,
    },
}

impl Work {
    fn from_change(change: Change) -> Result<Self, String> {
        Ok(match change {
            Change::DeleteAccount { name, opened } => Self::DeleteAccount { name, opened },
            Change::DeleteProject { town } => Self::DeleteProject { town },
            Change::RenameGroup { owner, from, to } => Self::RenameGroup { owner, from, to },
            Change::SetRole { name, visitor } => Self::SetRole { name, visitor },
            _ => return Err("this management change is not submitted work".into()),
        })
    }

    fn change(&self) -> Change {
        match self {
            Self::DeleteAccount { name, opened } => Change::DeleteAccount { name: name.clone(), opened: *opened },
            Self::DeleteProject { town } => Change::DeleteProject { town: town.clone() },
            Self::RenameGroup { owner, from, to } => {
                Change::RenameGroup { owner: owner.clone(), from: from.clone(), to: to.clone() }
            }
            Self::SetRole { name, visitor } => Change::SetRole { name: name.clone(), visitor: *visitor },
        }
    }

    fn unit(&self) -> String {
        match self {
            Self::DeleteAccount { name, .. } | Self::SetRole { name, .. } => format!("account/{name}"),
            Self::DeleteProject { town } => format!("project/{town}"),
            Self::RenameGroup { owner, .. } => format!("groups/{owner}"),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Asked {
    from: MachineName,
    key: PublicKey,
    account: AccountName,
    work: Work,
    /// When the account that asked was opened, as it asked: done only while
    /// it is that account, never one opened again under its name since
    /// (codex-d515b194). None in work kept from before this was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    opened: Option<jiff::Timestamp>,
}

/// Self-removal cannot retrieve a receipt through the identity it removes.
/// Dissolution and account deletion that removes the asker stay synchronous.
pub(crate) fn handles(change: &Change) -> bool {
    matches!(
        change,
        Change::DeleteAccount { .. }
            | Change::DeleteProject { .. }
            | Change::RenameGroup { .. }
            | Change::SetRole { .. }
    )
}

fn removes_origin(
    directory: &fastctx_net::directory::Directory,
    origin: &crate::origin::Origin,
    change: &Change,
) -> bool {
    matches!(change, Change::DeleteAccount { name, .. }
        if directory.member(&origin.name).is_some_and(|member| member.home.as_ref() == Some(name)))
}

const REASK: &str = "the account deletion was asked by an earlier version: nothing was done, please ask again";

/// Whether `account` is as it was opened when it asked, `opened`.
fn asked_as(store: &crate::store::Store, account: &AccountName, opened: Option<jiff::Timestamp>) -> bool {
    store.book.directory.account(account).map(|found| found.opened) == opened
}

fn reopened(account: &AccountName) -> String {
    format!("the account {account} that asked was opened again since: nothing was done")
}

const OWNERSHIP_CHANGED: &str =
    "the asking machine's ownership changed; nothing was done, please make the request again";

pub(crate) fn register(hub: &Arc<Hub>) {
    crate::submitted::register(hub, FAMILY, |hub: Arc<Hub>, asked: Asked| async move {
        hub.blocking(move |hub| {
            let Asked { from, key, account, work, opened } = asked;
            let origin = crate::origin::Origin { name: from, key };
            // Asked before the asking account's opening was kept with what
            // it asks, a deletion is asked again (codex-d515b194).
            if opened.is_none() && matches!(work, Work::DeleteAccount { .. }) {
                return Err(Failure::not_run(REASK));
            }
            let mut not_run = None;
            // Judged as the change is written, under the directory's writer:
            // the account that asked may be opened again meanwhile, and its
            // machine logged in to it anew (codex-d515b194).
            let (answer, ended) = super::change_checked(&hub, &origin, &account, work.change(), |store, change| {
                let why = if removes_origin(&store.book.directory, &origin, change) {
                    OWNERSHIP_CHANGED.to_string()
                } else if opened.is_some() && !asked_as(store, &account, opened) {
                    reopened(&account)
                } else {
                    return Ok(());
                };
                not_run = Some(why.clone());
                Err(why)
            });
            if let Some(why) = not_run {
                return Err(Failure::not_run(why));
            }
            super::end(&hub, ended);
            crate::updates::prefetch(&hub);
            crate::updates::tell(&hub, None);
            crate::updates::prune(&hub);
            match answer.map_err(Failure::new)? {
                Told::Done => Ok(()),
                _ => Err(Failure::new("the management change answered unexpectedly")),
            }
        })
        .await
    });
}

pub(crate) async fn change(
    hub: &Arc<Hub>,
    origin: crate::origin::Origin,
    account: AccountName,
    change: Change,
    named: Option<&Submission>,
    recorded: impl FnOnce(),
) -> (Result<Told, String>, Option<Vec<(MachineName, Refusal)>>) {
    let work = match Work::from_change(change) {
        Ok(work) => work,
        Err(why) => return (Err(why), None),
    };
    let opened = hub.store().book.directory.account(&account).map(|found| found.opened);
    let asked = Asked { from: origin.name.clone(), key: origin.key, account: account.clone(), work, opened };
    let identity = match serde_json::to_vec(&asked) {
        Ok(identity) => identity,
        Err(error) => return (Err(error.to_string()), None),
    };
    let synchronous = Failure::not_run("this account deletion must answer on its asking connection");
    let mut refusal = None;
    let make = async || {
        let change = asked.work.change();
        // Existing names are resolved before make: a same-name requery must
        // reach its receipt even if ownership changed while the job waited.
        if removes_origin(&hub.store().book.directory, &origin, &change) {
            return Err(synchronous.clone());
        }
        let (checking, source, actor) = (change.clone(), origin.clone(), account.clone());
        let admitted = hub.blocking(move |hub| super::admission::check(&hub, &source, &actor, &checking)).await;
        if let Err(why) = admitted {
            refusal = Some(why.clone());
            return Err(Failure::not_run(why));
        }
        Ok((asked.work.unit(), &asked))
    };
    let came: Result<(), Failure> =
        fastctx_jobs::submitted::submit(hub.as_ref(), &FAMILY, &identity, named, make, recorded).await;
    if let Some(why) = refusal
        && came.as_ref().is_err_and(|failure| *failure == Failure::not_run(why.clone()))
    {
        let why = super::admission::refused(hub, origin.clone(), account.clone(), asked.work.change(), why).await;
        return (Err(why), None);
    }
    if came == Err(synchronous) {
        let (change, opened) = (asked.work.change(), asked.opened);
        // As the change is written, by the account that asked as it was
        // opened then (codex-d515b194).
        let (answer, ended) = hub
            .blocking(move |hub| {
                super::change_checked(&hub, &origin, &account, change, |store, _| {
                    if asked_as(store, &account, opened) { Ok(()) } else { Err(reopened(&account)) }
                })
            })
            .await;
        return (answer, Some(ended));
    }
    (crate::submitted::told(came.map(|()| Told::Done)), None)
}

/// An account's model collaboration written under a name (H10): written once
/// however often it is asked again, the version it made answering each
/// asking. A write whose answer was lost and that is asked again gets that
/// version, not a refusal saying the version read is behind, which made
/// the asker make its change again over its own and undo a choice it
/// toggled.
const COLLAB: Family = Family {
    name: "ask/collab",
    at_once: 4,
    within: Duration::from_secs(60),
    stays: Duration::from_secs(45),
    // The write compares the version it was read at: done again, it would
    // be refused, so it is not.
    again: false,
};

#[derive(Serialize, Deserialize)]
struct CollabAsked {
    from: MachineName,
    key: PublicKey,
    account: AccountName,
    collab: fastctx_types::collab::Collab,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    opened: Option<jiff::Timestamp>,
}

/// Registers the work of `collab`.
pub(crate) fn register_collab(hub: &Arc<Hub>) {
    crate::submitted::register(hub, COLLAB, |hub: Arc<Hub>, asked: CollabAsked| async move {
        hub.blocking(move |hub| {
            let CollabAsked { from, key, account, collab, opened } = asked;
            let origin = crate::origin::Origin { name: from, key };
            let (answer, ended) =
                super::change_checked(&hub, &origin, &account, Change::Collab { collab }, |store, _| {
                    if asked_as(store, &account, opened) { Ok(()) } else { Err(reopened(&account)) }
                });
            super::end(&hub, ended);
            // Refused, nothing was written.
            match answer.map_err(Failure::not_run)? {
                Told::CollabWritten { version } => Ok(version),
                _ => Err(Failure::new("the collaboration change answered unexpectedly")),
            }
        })
        .await
    });
}

/// Writes `collab` for `account`, as `origin` asked under `named`.
pub(crate) async fn collab(
    hub: &Arc<Hub>,
    origin: crate::origin::Origin,
    account: AccountName,
    collab: fastctx_types::collab::Collab,
    named: Option<&Submission>,
) -> Result<Told, String> {
    let opened = hub.store().book.directory.account(&account).map(|found| found.opened);
    let asked = CollabAsked { from: origin.name.clone(), key: origin.key, account: account.clone(), collab, opened };
    let identity = serde_json::to_vec(&asked).map_err(|error| error.to_string())?;
    let make = async || {
        let (checking, source, actor) =
            (Change::Collab { collab: asked.collab.clone() }, origin.clone(), account.clone());
        hub.blocking(move |hub| super::admission::check(&hub, &source, &actor, &checking))
            .await
            .map_err(Failure::not_run)?;
        Ok((format!("collab/{account}"), &asked))
    };
    let came: Result<u64, Failure> =
        fastctx_jobs::submitted::submit(hub.as_ref(), &COLLAB, &identity, named, make, || {}).await;
    crate::submitted::told(came.map(|version| Told::CollabWritten { version }))
}
