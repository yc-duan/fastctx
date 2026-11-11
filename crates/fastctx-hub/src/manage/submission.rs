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
    DeleteProject { town: TownId },

    SetRole { name: AccountName, visitor: bool },
}

impl Work {
    fn from_change(change: Change) -> Result<Self, String> {
        Ok(match change {
            Change::DeleteProject { town } => Self::DeleteProject { town },

            Change::SetRole { name, visitor } => Self::SetRole { name, visitor },
            _ => return Err("this management change is not submitted work".into()),
        })
    }

    fn change(&self) -> Change {
        match self {
            Self::DeleteProject { town } => Change::DeleteProject { town: town.clone() },

            Self::SetRole { name, visitor } => Change::SetRole { name: name.clone(), visitor: *visitor },
        }
    }

    fn unit(&self) -> String {
        match self {
            Self::SetRole { name, .. } => format!("account/{name}"),
            Self::DeleteProject { town } => format!("project/{town}"),
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

pub(crate) fn handles(change: &Change) -> bool {
    matches!(change, Change::DeleteProject { .. } | Change::SetRole { .. })
}

pub(crate) fn register(hub: &Arc<Hub>) {
    crate::submitted::register(hub, FAMILY, |hub: Arc<Hub>, asked: Asked| async move {
        hub.blocking(move |hub| {
            let Asked { from, key, account, work } = asked;
            let origin = crate::origin::Origin { name: from, key };
            let (answer, ended) = super::change(&hub, &origin, &account, work.change());
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
) -> Result<Told, String> {
    let work = Work::from_change(change)?;
    let asked = Asked { from: origin.name.clone(), key: origin.key, account: account.clone(), work };
    let identity = serde_json::to_vec(&asked).map_err(|error| error.to_string())?;
    let make = || async {
        let change = asked.work.change();
        if let Err(why) = super::admission::check(hub, &origin, &account, &change) {
            let why = super::admission::refused(hub, origin.clone(), account.clone(), change, why).await;
            return Err(Failure::new(why));
        }
        Ok((asked.work.unit(), &asked))
    };
    let came: Result<(), Failure> =
        fastctx_jobs::submitted::submit(hub.as_ref(), &FAMILY, &identity, named, make, || {}).await;
    crate::submitted::told(came.map(|()| Told::Done))
}
