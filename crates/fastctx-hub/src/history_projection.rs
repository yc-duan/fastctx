//! Reader-visible provenance includes conversations they own even on an assisting host.

use fastctx_types::{AccountName, history::HistoryRevision, project::Writer};

pub(crate) fn revision(
    hub: &crate::Hub,
    directory: &fastctx_net::directory::Directory,
    actor: &AccountName,
    revision: &mut HistoryRevision,
) {
    let own_conversation = match &revision.author.stamp.by {
        Writer::Resident { address } => {
            hub.account_records.owners.lock().expect("never poisoned").resolve(directory, actor, address).is_ok()
        }
        _ => false,
    };
    let writer = revision.author.stamp.by.clone();
    revision
        .author
        .stamp
        .for_reader(&|machine| directory.authorize(actor, fastctx_net::authorization::Action::Work(machine)).is_ok());
    if own_conversation {
        revision.author.stamp.by = writer;
    }
}
