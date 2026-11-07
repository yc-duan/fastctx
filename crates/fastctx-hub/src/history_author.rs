//! Checks new history attribution without rewriting signed or offline revisions.

use fastctx_net::{authorization::Action, sign::PublicKey};
use fastctx_types::{AccountName, MachineName, history::Author, project::Writer};
use jiff::Timestamp;

pub(crate) fn check(
    hub: &crate::Hub,
    store: &crate::store::Snapshot,
    from: &MachineName,
    key: &PublicKey,
    actor: &AccountName,
    author: &Author,
) -> Result<(), String> {
    let directory = store.indexed();
    if directory.member(from).is_none_or(|member| member.key != *key) {
        return Err("the history writer's device was replaced".into());
    }
    directory.authorize(actor, Action::ActFor(from)).map_err(|error| error.to_string())?;
    if author.stamp.on.as_ref().is_none_or(|on| !directory.same(on, from)) {
        return Err("the history author must name the device sending this revision".into());
    }
    // Offline drafts retain their edit time when their device reconnects.
    if author.stamp.at > Timestamp::now().checked_add(jiff::SignedDuration::from_secs(300)).unwrap_or(Timestamp::MAX) {
        return Err("the history author's time is more than 5 minutes ahead of the hub's clock".into());
    }
    match &author.stamp.by {
        Writer::User => Ok(()),
        Writer::Resident { address } => {
            let owners = hub.account_records.owners.lock().expect("never poisoned");
            let conversation = owners
                .resolve(&directory, actor, address)
                .map_err(|_| "the history author is not a registered conversation for this account")?;
            if &conversation.account != actor {
                return Err("the history author's conversation belongs to a different account".into());
            }
            Ok(())
        }
        _ => Err("only a user or a registered conversation can author a history revision".into()),
    }
}
