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
    submitted: bool,
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
    let now = Timestamp::now();
    if author.stamp.at > now.checked_add(jiff::SignedDuration::from_secs(300)).unwrap_or(Timestamp::MAX) {
        return Err("the history author's time is more than 5 minutes ahead of the hub's clock".into());
    }
    match &author.stamp.by {
        Writer::User | Writer::Page { .. } => Ok(()),
        Writer::Session { .. } => legacy(author, submitted, now),
        Writer::Resident { address } => {
            let owners = hub.account_records.owners.lock().expect("never poisoned");
            if !owners.registered_at(&directory, address) {
                return legacy(author, submitted, now);
            }
            let conversation = owners
                .resolve(&directory, actor, address)
                .map_err(|_| "the history author is not a registered conversation for this account")?;
            if &conversation.account != actor {
                return Err("the history author's conversation belongs to a different account".into());
            }
            Ok(())
        }
        Writer::Elsewhere { .. } => Err("a history revision must name its original author".into()),
    }
}

fn legacy(author: &Author, submitted: bool, now: Timestamp) -> Result<(), String> {
    if !submitted {
        return Err("an old history author requires a submission identity".into());
    }
    if author.stamp.at > now {
        return Err("a replayed history revision cannot have a future edit time".into());
    }
    Ok(())
}
