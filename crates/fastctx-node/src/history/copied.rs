//! A conversation's notes as the copy here holds them (plan 3-5, design
//! §28.27): the hub orders each note's latest revision into the
//! conversation's scope by its name (`history` stream, `hub
//! assets::mirror`). The list is answered from it first, while it is as
//! fresh as anything seen here of the notes (design §15 `switch`); the
//! latest revision of each note while the hub is not reached. Earlier
//! revisions, and the list of them a note is read with, are the hub's
//! alone: a note read here offers its latest as all there is, and asks for
//! another go to the hub, as writes do.

use crate::Node;
use crate::timeline::copied_records::{copy_of, fresh_records, record_in, records_in};
use fastctx_content::v2::{ConversationHistory, Kind as _};
use fastctx_types::access::Acting;
use fastctx_types::account_records::Record;
use fastctx_types::history::stored::Stored;
use fastctx_types::history::{Catalog, Document, HistoryCall, HistoryReply, RECORD_KIND, Scope};
use std::sync::Arc;

/// `request`, a read of a conversation's notes, as the copy here holds them
/// for `acting`; none when it writes, the hub is reached, or the copy cannot
/// answer it.
pub(crate) async fn answer(node: &Arc<Node>, acting: &Acting, request: &HistoryCall) -> Option<HistoryReply> {
    if request.writes() {
        return None;
    }
    let (node, acting, request) = (node.clone(), acting.clone(), request.clone());
    fastctx_platform::task::spawn_blocking(move || answered(&node, &acting, &request, false)).await.ok().flatten()
}

/// `request`, a list of a conversation's notes, as the copy here holds it
/// while it is as fresh as anything seen here of them, the hub reached or
/// not; none otherwise, and for any other request.
pub(crate) async fn answer_first(node: &Arc<Node>, acting: &Acting, request: &HistoryCall) -> Option<HistoryReply> {
    if !matches!(request, HistoryCall::List { .. }) {
        return None;
    }
    let (node, acting, request) = (node.clone(), acting.clone(), request.clone());
    fastctx_platform::task::spawn_blocking(move || answered(&node, &acting, &request, true)).await.ok().flatten()
}

fn answered(node: &Node, acting: &Acting, request: &HistoryCall, first: bool) -> Option<HistoryReply> {
    let Scope::Conversation { resident, machine } = request.scope() else { return None };
    let library = &node.assets.as_ref()?.library;
    let scope = copy_of(node, resident, Some(machine), None, None)?;
    let away = crate::timeline::copy::hub_away(node, &scope);
    if !away && !first {
        return None;
    }
    // A note as the hub keeps it: the conversation's, live, whole.
    let stored = |record: Record| -> Option<Stored> {
        if record.metadata.deleted || record.metadata.kind != RECORD_KIND {
            return None;
        }
        let (owner, _) = record.metadata.key.split_once('/')?;
        if owner != resident.as_str() {
            return None;
        }
        let stored: Stored = serde_json::from_value(record.body).ok()?;
        stored.validate().ok()?;
        Some(stored)
    };
    let mut reply = match request {
        HistoryCall::List { .. } => {
            let records = if away {
                records_in(library, &scope.key, ConversationHistory::STREAM)?
            } else {
                let prefix = format!("{resident}/");
                fresh_records(node, &scope, ConversationHistory::STREAM, RECORD_KIND, &prefix)?
            };
            let mut catalog =
                Catalog { documents: records.into_iter().filter_map(stored).map(|stored| stored.revision).collect() };
            catalog.sort();
            HistoryReply::Catalog { catalog }
        }
        HistoryCall::Read { name, version, .. } => {
            let stored = stored(record_in(library, &scope.key, ConversationHistory::STREAM, name)??)?;
            if stored.revision.name != *name || version.is_some_and(|version| version != stored.revision.version) {
                return None;
            }
            let versions = vec![stored.revision.clone()];
            HistoryReply::Document {
                document: Box::new(Document { revision: stored.revision, text: stored.text, versions }),
            }
        }
        _ => return None,
    };
    // Who wrote it, as the hub would show it: the machines the accounts here
    // may use by hand named, the rest not.
    let link = node.links().into_iter().find(|link| Some(link.membership().id.as_str()) == scope.hub.as_deref())?;
    let (accounts, view) = (node.accounts_on(&link, acting), link.view());
    let usable =
        |machine: &fastctx_types::MachineName| accounts.iter().any(|account| view.rights(account, machine).hands);
    match &mut reply {
        HistoryReply::Catalog { catalog } => catalog.for_reader(&usable),
        HistoryReply::Document { document } => document.for_reader(&usable),
        HistoryReply::Saved { .. } => {}
    }
    Some(reply)
}
