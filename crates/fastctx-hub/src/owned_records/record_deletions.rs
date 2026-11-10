//! Record deletion retains its original comparison and resolved account incarnation.

use super::*;
use crate::submitted::Family;
use fastctx_types::{ipc::Failure, submission::Submission};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::time::Duration;

const FAMILY: Family = Family {
    name: "ask/delete-record",
    at_once: 4,
    within: Duration::from_secs(10 * 60),
    // Every individual question to the hub has a one-minute deadline.
    stays: Duration::from_secs(45),
    // Publishing advances the revision. Repeating the original comparison
    // would conflict instead of finishing a partly erased version directory.
    again: false,
};

#[derive(Serialize, Deserialize)]
struct Asked {
    from: MachineName,
    key: PublicKey,
    account: AccountName,
    conversation: Option<Address>,
    request: RecordRequest,
}

/// Only the facts used by the record gate. Address aliases can grow with
/// every move, and neither they nor the canonical object ID grant access.
#[derive(Clone, Serialize, Deserialize)]
struct Ownership {
    actor: AccountName,
    account: AccountName,
    address: Address,
    shared_on: Option<MachineName>,
    born_seal: Option<String>,
}

impl From<Access> for Ownership {
    fn from(access: Access) -> Self {
        let Access { actor, conversation } = access;
        // The owning account needs no shared-machine grant. For another
        // actor, check already verified the bounded seal of the birth machine.
        let shared = actor != conversation.account;
        Self {
            actor,
            account: conversation.account,
            address: conversation.address,
            shared_on: shared.then_some(conversation.shared_on).flatten(),
            born_seal: shared.then_some(conversation.born_seal).flatten(),
        }
    }
}

impl Ownership {
    fn access(&self) -> Access {
        Access {
            actor: self.actor.clone(),
            conversation: Conversation {
                account: self.account.clone(),
                address: self.address.clone(),
                shared_on: self.shared_on.clone(),
                born_seal: self.born_seal.clone(),
                previous: Vec::new(),
                identity: None,
            },
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Recorded {
    asked: Asked,
    opened: Timestamp,
    access: Option<Ownership>,
}

#[derive(Serialize, Deserialize)]
struct Answered {
    opened: Timestamp,
    access: Option<Ownership>,
    told: Told,
}

pub(crate) fn register(hub: &Arc<Hub>) {
    crate::submitted::register(hub, FAMILY, |hub: Arc<Hub>, recorded: Recorded| async move {
        let Recorded { asked, opened, access } = recorded;
        let Asked { from, key, account, conversation: _, request } = asked;
        let owner = access.as_ref().map_or(&account, |access| &access.account).clone();
        let gate = access.as_ref().map(Ownership::access);
        if let Some(access) = &gate {
            super::acts_for(&hub, &from, &key, &account).map_err(Failure::new)?;
            super::check(&hub, &hub.store(), &from, &key, &owner, opened, access).map_err(Failure::new)?;
        } else {
            account_records::begin(&hub, &from, &key, &account, &request).map_err(Failure::new)?;
            account_records::authorized(&hub, &from, &key, &owner, opened).map_err(Failure::new)?;
        }
        let told =
            account_records::ask_as(&hub, from, key, owner, opened, request, gate).await.map_err(Failure::new)?;
        Ok(Answered { opened, access, told })
    });
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn account(
    hub: &Arc<Hub>,
    from: MachineName,
    key: PublicKey,
    account: AccountName,
    opened: Timestamp,
    request: RecordRequest,
    named: Option<&Submission>,
) -> Result<Told, String> {
    // The control entrance already called begin, including validation and
    // the legacy plan guard, before capturing this account incarnation.
    account_records::authorized(hub, &from, &key, &account, opened)?;
    submit(hub, Asked { from, key, account, conversation: None, request }, opened, None, named).await
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn owned(
    hub: &Arc<Hub>,
    from: MachineName,
    key: PublicKey,
    account: AccountName,
    conversation: Address,
    request: RecordRequest,
    named: Option<&Submission>,
) -> Result<Told, String> {
    super::acts_for(hub, &from, &key, &account)?;
    request.validate()?;
    let (access, opened) = super::resolve(hub, &account, &conversation)?;
    let RecordRequest::Delete { kind, key: record_key, .. } = &request else {
        return Err("the record deletion received another kind of request".into());
    };
    super::record_key(&access.conversation, kind, record_key)?;
    super::check(hub, &hub.store(), &from, &key, &access.conversation.account, opened, &access)?;
    submit(
        hub,
        Asked { from, key, account, conversation: Some(conversation), request },
        opened,
        Some(access.into()),
        named,
    )
    .await
}

async fn submit(
    hub: &Arc<Hub>,
    asked: Asked,
    opened: Timestamp,
    access: Option<Ownership>,
    named: Option<&Submission>,
) -> Result<Told, String> {
    let RecordRequest::Delete { kind, key, .. } = &asked.request else {
        return Err("the record deletion received another kind of request".into());
    };
    let owner = access.as_ref().map_or(&asked.account, |access| &access.account);
    let resource = serde_json::to_vec(&(owner, opened, kind, key)).map_err(|error| error.to_string())?;
    // A valid 256-byte record key still fits the queue's 256-byte unit bound.
    let unit = format!("record/{:x}", Sha256::digest(resource));
    // What was resolved belongs to the first recording, but not to its
    // identity: a later asking cannot create another operation by resolving
    // the same account or address to a different incarnation.
    let identity = serde_json::to_vec(&asked).map_err(|error| error.to_string())?;
    let (from, member_key, actor) = (asked.from.clone(), asked.key, asked.account.clone());
    let recorded = Recorded { asked, opened, access };
    let came = fastctx_jobs::submitted::submit(hub.as_ref(), &FAMILY, &unit, &recorded, &identity, named, || {}).await;
    let Answered { opened, access, told } = match came {
        Ok(answered) => answered,
        Err(failure) => return crate::submitted::told(Err(failure)),
    };
    if let Some(access) = access {
        let access = access.access();
        super::check(hub, &hub.store(), &from, &member_key, &access.conversation.account, opened, &access)?;
        let Told::AccountRecords { response } = told else {
            return Err("the record deletion received an unexpected reply".into());
        };
        let mut response = Response::Record { response };
        // A retained receipt never grants the provenance its reader lost.
        super::projection::redact(hub, &actor, &mut response)?;
        Ok(Told::OwnedRecords { response })
    } else {
        account_records::authorized(hub, &from, &member_key, &actor, opened)?;
        Ok(told)
    }
}
