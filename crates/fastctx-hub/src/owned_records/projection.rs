//! Shared records disclose provenance only where the acting reader may work.

use super::*;
use fastctx_types::account_records::{Metadata, Record};

fn metadata(value: &mut Metadata, usable: &dyn Fn(&MachineName) -> bool) {
    if value.by.as_ref().is_some_and(|machine| !usable(machine)) {
        value.by = None;
    }
}

fn record(
    hub: &Hub,
    directory: &fastctx_net::directory::Directory,
    actor: &AccountName,
    value: &mut Record,
    usable: &dyn Fn(&MachineName) -> bool,
) -> Result<(), String> {
    metadata(&mut value.metadata, usable);
    if !value.metadata.deleted && value.metadata.kind == fastctx_types::history::RECORD_KIND {
        let mut stored: fastctx_types::history::stored::Stored =
            serde_json::from_value(value.body.take()).map_err(|error| error.to_string())?;
        crate::history_projection::revision(hub, directory, actor, &mut stored.revision);
        value.body = serde_json::to_value(stored).map_err(|error| error.to_string())?;
    }
    if !value.metadata.deleted && value.metadata.kind == fastctx_types::plan::KIND {
        let mut body: fastctx_types::plan::Body =
            serde_json::from_value(value.body.take()).map_err(|error| error.to_string())?;
        if matches!(&body.author, fastctx_types::plan::Author::Resident { address } if !usable(&address.machine)) {
            body.author = fastctx_types::plan::Author::Elsewhere;
        }
        value.body = serde_json::to_value(body).map_err(|error| error.to_string())?;
    }
    Ok(())
}

pub(super) fn redact(hub: &Hub, actor: &AccountName, response: &mut Response) -> Result<(), String> {
    let store = hub.store();
    let usable = |machine: &MachineName| store.book.directory.authorize(actor, Action::Work(machine)).is_ok();
    match response {
        Response::PlanHead { metadata: Some(value), .. } => metadata(value, &usable),
        Response::Record { response } => match response {
            RecordResponse::Read { record: Some(value) } => record(hub, &store.book.directory, actor, value, &usable)?,
            RecordResponse::Listed { entries } => entries.iter_mut().for_each(|value| metadata(value, &usable)),
            RecordResponse::Written { metadata: value } | RecordResponse::Conflict { current: Some(value) } => {
                metadata(value, &usable)
            }
            _ => {}
        },
        Response::Revision { record: value } | Response::Submitted { record: Some(value) } => {
            record(hub, &store.book.directory, actor, value, &usable)?
        }
        Response::Versions { versions } => {
            for version in versions {
                metadata(&mut version.metadata, &usable);
                if let Some(history) = &mut version.history {
                    crate::history_projection::revision(hub, &store.book.directory, actor, history);
                }
            }
        }
        Response::Catalog { catalog } => {
            for revision in &mut catalog.documents {
                crate::history_projection::revision(hub, &store.book.directory, actor, revision);
            }
        }
        Response::Replicas { catalog } => {
            for head in &mut catalog.documents {
                if let Some(revision) = &mut head.history {
                    crate::history_projection::revision(hub, &store.book.directory, actor, revision);
                }
            }
        }
        _ => {}
    }
    Ok(())
}
