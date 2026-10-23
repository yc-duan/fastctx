//! A remote job returns plan operations only to the conversation it worked for.

use crate::{Hub, store::Store};
use fastctx_net::authorization::Action;
use fastctx_types::{AccountName, Address, MachineName, ResidentId, plan::words};

pub(crate) fn check(
    hub: &Hub,
    store: &Store,
    from: &MachineName,
    account: &AccountName,
    to: &MachineName,
    resident: &ResidentId,
) -> Result<(), String> {
    store.book.directory.authorize(account, Action::Work(from)).map_err(|error| error.to_string())?;
    if !store.worked.contains(&(from.clone(), Address { machine: to.clone(), resident: resident.clone() })) {
        return Err(words::JOB.into());
    }
    if hub.sessions().get(to).is_some_and(|session| !session.conversation_plans) {
        return Err(words::OLDER.into());
    }
    Ok(())
}
