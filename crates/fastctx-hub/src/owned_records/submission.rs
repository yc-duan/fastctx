//! Deleting a conversation's records is work the hub is asked for, not an
//! answer it has: every plan and history document of the conversation goes,
//! with every version each ever had, however many those are. The question
//! records the deletion and is answered when it is done, if that is soon;
//! the deletion goes on to its end whether or not the member that asked is
//! still there, and asked again under its name it is the same deletion.

use super::*;
use crate::submitted::Family;
use fastctx_types::ResidentId;
use fastctx_types::ipc::Failure;
use fastctx_types::submission::Submission;
use serde::{Deserialize, Serialize};
use std::time::Duration;

const FAMILY: Family = Family {
    name: "ask/delete-conversation-records",
    at_once: 4,
    within: Duration::from_secs(10 * 60),
    // Under the minute a member waits for the hub's answer.
    stays: Duration::from_secs(45),
    // The deletion leaves a tombstone first and removes what is still
    // there: done again after a try that did not finish, it does the rest.
    again: true,
};

#[derive(Serialize, Deserialize)]
struct Asked {
    from: MachineName,
    key: PublicKey,
    account: AccountName,
    resident: ResidentId,
}

pub(crate) fn register(hub: &Arc<Hub>) {
    crate::submitted::register(hub, FAMILY, |hub: Arc<Hub>, asked: Asked| async move {
        let Asked { from, key, account, resident } = asked;
        // As the question would have been answered: whether the member may
        // still ask is seen again as the work begins.
        super::ask(&hub, from, key, account, Request::Delete { resident }).await.map_err(Failure::new)
    });
}

pub(super) async fn delete(
    hub: &Arc<Hub>,
    from: MachineName,
    key: PublicKey,
    account: AccountName,
    resident: ResidentId,
    named: Option<&Submission>,
) -> Result<Told, String> {
    // Who may not ask is told so now, with nothing recorded.
    super::acts_for(hub, &from, &key, &account)?;
    let unit = format!("{account}/{resident}");
    let asked = Asked { from, key, account, resident };
    crate::submitted::told(crate::submitted::submit(hub, &FAMILY, &unit, &asked, named).await)
}
