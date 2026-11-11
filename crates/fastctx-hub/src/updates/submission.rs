//! Batch arrangements retain their original per-machine answers and transactions.

use crate::{Hub, origin::Origin, submitted::Family};
use fastctx_net::{sign::PublicKey, wire::Told};
use fastctx_types::update::{Scheduled, ViewCall, ViewReply};
use fastctx_types::{AccountName, MachineName, ipc::Failure, submission::Submission};
use serde::{Deserialize, Serialize};
use std::{sync::Arc, time::Duration};

const FAMILY: Family = Family {
    name: "ask/network-updates",
    at_once: 4,
    within: Duration::from_secs(10 * 60),
    stays: Duration::from_secs(45),
    // Completed chunks survive a crash, but the original request has no
    // checkpoint for later chunks. Repeating can replace newer arrangements.
    again: false,
};

#[derive(Serialize, Deserialize)]
enum Work {
    Schedule { machines: Vec<MachineName>, version: Option<String> },
    Repair { machines: Vec<MachineName>, version: Option<String> },
    CancelRepair { machines: Vec<MachineName> },
}

impl Work {
    fn from_call(call: ViewCall) -> Result<Self, String> {
        Ok(match call {
            ViewCall::Schedule { machines, version } => Self::Schedule { machines, version },
            ViewCall::Repair { machines, version } => Self::Repair { machines, version },
            ViewCall::CancelRepair { machines } => Self::CancelRepair { machines },
            _ => return Err("this update request is not submitted work".into()),
        })
    }

    fn call(self) -> ViewCall {
        match self {
            Self::Schedule { machines, version } => ViewCall::Schedule { machines, version },
            Self::Repair { machines, version } => ViewCall::Repair { machines, version },
            Self::CancelRepair { machines } => ViewCall::CancelRepair { machines },
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

pub(crate) fn handles(call: &ViewCall) -> bool {
    matches!(call, ViewCall::Schedule { .. } | ViewCall::Repair { .. } | ViewCall::CancelRepair { .. })
}

pub(crate) fn register(hub: &Arc<Hub>) {
    crate::submitted::register(hub, FAMILY, |hub: Arc<Hub>, asked: Asked| async move {
        let origin = Origin { name: asked.from, key: asked.key };
        match super::view_request(&hub, &origin, &asked.account, asked.work.call()).await.map_err(Failure::new)? {
            Told::NetworkUpdates { reply: ViewReply::Scheduled { results } } => Ok(results),
            _ => Err(Failure::new("the update arrangement answered unexpectedly")),
        }
    });
}

pub(crate) async fn request(
    hub: &Arc<Hub>,
    origin: Origin,
    account: AccountName,
    call: ViewCall,
    named: Option<&Submission>,
) -> Result<Told, String> {
    // The same authority protects both admission and a retained per-machine
    // result. Version and target refusals remain rows in the original answer.
    super::view::authenticate(&hub.store().book.directory, &origin, &account)?;
    let asked = Asked { from: origin.name, key: origin.key, account, work: Work::from_call(call)? };
    let identity = serde_json::to_vec(&asked).map_err(|error| error.to_string())?;
    // One account's arrangements keep its submission order; other accounts
    // keep their independent queue units and the original per-chunk checks.
    let unit = format!("updates/{}", asked.account);
    let make = || async { Ok((unit, &asked)) };
    let came: Result<Vec<Scheduled>, Failure> =
        fastctx_jobs::submitted::submit(hub.as_ref(), &FAMILY, &identity, named, make, || {}).await;
    if came.is_ok() {
        let origin = Origin { name: asked.from, key: asked.key };
        super::view::authenticate(&hub.store().book.directory, &origin, &asked.account)?;
    }
    crate::submitted::told(came.map(|results| Told::NetworkUpdates { reply: ViewReply::Scheduled { results } }))
}
