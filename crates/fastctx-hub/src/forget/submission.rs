//! Preparing a deletion's unbounded audience is work, before its existing delivery protocol.

use super::*;
use crate::submitted::Family;
use fastctx_net::sign::PublicKey;
use fastctx_types::{ipc::Failure, submission::Submission};
use std::{sync::Arc, time::Duration};

const FAMILY: Family = Family {
    name: "ask/forget",
    at_once: 8,
    within: Duration::from_secs(10 * 60),
    stays: Duration::from_secs(45),
    // A pending deletion is repeatable, but one that already finished would
    // allocate another deletion and audit. Keep uncertain acceptance explicit.
    again: false,
};

#[derive(Serialize, Deserialize)]
struct Asked {
    from: MachineName,
    key: PublicKey,
    account: AccountName,
    conversation: Address,
}

/// The original pre-mutation checks, without constructing the audience or
/// taking the catalog writer. No lock or account condition is added here.
fn check(hub: &Arc<Hub>, asked: &Asked) -> Result<String, String> {
    let origin = crate::origin::Origin { name: asked.from.clone(), key: asked.key };
    let conversation = {
        let store = hub.store();
        let directory = &store.book.directory;
        origin.check(directory)?;
        if !(directory.logged_in(&asked.from, &asked.account)
            || directory.same(&asked.from, &asked.conversation.machine)
                && directory.may_act_for(&asked.account, &asked.from))
        {
            return Err(format!("{} is not logged in to the account {}", asked.from, asked.account));
        }
        let home = directory
            .member(&asked.conversation.machine)
            .filter(|member| member.is_machine())
            .ok_or_else(|| format!("there is no machine named {} on the hub", asked.conversation.machine))?;
        Address { machine: home.name.clone(), resident: asked.conversation.resident.clone() }
    };
    let ownership = super::ownership::resolve(hub, &asked.account, &conversation);
    let conversation = ownership.as_ref().map_or(conversation, |owner| owner.address.clone());
    let account = ownership.as_ref().map_or(&asked.account, |owner| &owner.account);
    if ownership.is_none() && crate::afoot::owned_by(hub, &conversation, account) == Some(false) {
        return Err(format!(
            "the conversation {} on {} does not work for the account {account}",
            conversation.resident, conversation.machine
        ));
    }
    if ownership
        .as_ref()
        .is_some_and(|owner| !crate::owned_records::allows(&hub.store().book.directory, &asked.account, owner))
    {
        return Err("this account no longer owns that conversation".into());
    }
    Ok(format!("forget/{account}/{}", conversation.resident))
}

pub(crate) fn register(hub: &Arc<Hub>) {
    crate::submitted::register(hub, FAMILY, |hub: Arc<Hub>, asked: Asked| async move {
        check(&hub, &asked).map_err(Failure::new)?;
        hub.blocking(move |hub| super::ask(&hub, &asked.from, asked.account, asked.conversation))
            .await
            .map_err(Failure::new)
    });
}

pub(crate) async fn ask(
    hub: &Arc<Hub>,
    origin: crate::origin::Origin,
    account: AccountName,
    conversation: Address,
    named: Option<&Submission>,
) -> Result<Told, String> {
    let asked = Asked { from: origin.name, key: origin.key, account, conversation };
    let identity = serde_json::to_vec(&asked).map_err(|error| error.to_string())?;
    let make = || async { Ok((check(hub, &asked).map_err(Failure::new)?, &asked)) };
    crate::submitted::told(fastctx_jobs::submitted::submit(hub.as_ref(), &FAMILY, &identity, named, make, || {}).await)
}
