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
    DeleteAccount { name: AccountName },
    DeleteProject { town: TownId },
    RenameGroup { owner: AccountName, from: String, to: Option<String> },
    SetRole { name: AccountName, visitor: bool },
}

impl Work {
    fn from_change(change: Change) -> Result<Self, String> {
        Ok(match change {
            Change::DeleteAccount { name } => Self::DeleteAccount { name },
            Change::DeleteProject { town } => Self::DeleteProject { town },
            Change::RenameGroup { owner, from, to } => Self::RenameGroup { owner, from, to },
            Change::SetRole { name, visitor } => Self::SetRole { name, visitor },
            _ => return Err("this management change is not submitted work".into()),
        })
    }

    fn change(&self) -> Change {
        match self {
            Self::DeleteAccount { name } => Change::DeleteAccount { name: name.clone() },
            Self::DeleteProject { town } => Change::DeleteProject { town: town.clone() },
            Self::RenameGroup { owner, from, to } => {
                Change::RenameGroup { owner: owner.clone(), from: from.clone(), to: to.clone() }
            }
            Self::SetRole { name, visitor } => Change::SetRole { name: name.clone(), visitor: *visitor },
        }
    }

    fn unit(&self) -> String {
        match self {
            Self::DeleteAccount { name } | Self::SetRole { name, .. } => format!("account/{name}"),
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
    matches!(change, Change::DeleteAccount { name }
        if directory.member(&origin.name).is_some_and(|member| member.home.as_ref() == Some(name)))
}

const OWNERSHIP_CHANGED: &str =
    "the asking machine's ownership changed; nothing was done, please make the request again";

pub(crate) fn register(hub: &Arc<Hub>) {
    crate::submitted::register(hub, FAMILY, |hub: Arc<Hub>, asked: Asked| async move {
        hub.blocking(move |hub| {
            let Asked { from, key, account, work } = asked;
            let origin = crate::origin::Origin { name: from, key };
            let mut not_run = false;
            let (answer, ended) = super::change_checked(&hub, &origin, &account, work.change(), |store, change| {
                if removes_origin(&store.book.directory, &origin, change) {
                    not_run = true;
                    return Err(OWNERSHIP_CHANGED.into());
                }
                Ok(())
            });
            if not_run {
                return Err(Failure::not_run(OWNERSHIP_CHANGED));
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
    let asked = Asked { from: origin.name.clone(), key: origin.key, account: account.clone(), work };
    let identity = match serde_json::to_vec(&asked) {
        Ok(identity) => identity,
        Err(error) => return (Err(error.to_string()), None),
    };
    let synchronous = Failure::not_run("this account deletion must answer on its asking connection");
    let mut refusal = None;
    let make = || async {
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
        let change = asked.work.change();
        let (answer, ended) = hub.blocking(move |hub| super::change(&hub, &origin, &account, change)).await;
        return (answer, Some(ended));
    }
    (crate::submitted::told(came.map(|()| Told::Done)), None)
}
