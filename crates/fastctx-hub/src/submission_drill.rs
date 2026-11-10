//! Fault injection and observations for the isolated submission drill only.

use crate::Hub;
use fastctx_net::wire::Told;
use fastctx_types::{ResidentId, ipc::Failure, submission::Submission};
use std::time::Duration;

/// A drill selects one conversation by writing its ID beside the hub's
/// diagnostics. Its deletion then crosses the family's 45-second stay.
pub(crate) async fn deleting(hub: &Hub, resident: &ResidentId) {
    let path = hub.diagnostic_dir.join("drill-slow-record-deletion");
    let selected = fastctx_platform::task::spawn_blocking(move || fastctx_platform::disk::read_to_string(&path))
        .await
        .ok()
        .and_then(Result::ok);
    if selected.as_deref().map(str::trim) == Some(resident.as_str()) {
        hub.log.line(format_args!("submission-drill delaying resident={resident} seconds=50"));
        tokio::time::sleep(Duration::from_secs(50)).await;
    }
}

/// The next question under this same name proves the member heard acceptance
/// and followed it, rather than submitting a second deletion.
pub(crate) fn answered(hub: &Hub, resident: &ResidentId, named: Option<&Submission>, came: &Result<Told, Failure>) {
    let name = named.map_or("unnamed", |named| named.name.as_str());
    let accepted = came.as_ref().is_err_and(|failure| failure.is_accepted());
    let done = matches!(came, Ok(Told::OwnedRecords { response: fastctx_types::owned_records::Response::Deleted }));
    hub.log.line(format_args!(
        "submission-drill answered resident={resident} name={name} accepted={accepted} done={done}"
    ));
}
