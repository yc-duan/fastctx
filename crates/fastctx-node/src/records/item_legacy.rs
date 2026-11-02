//! Queue files follow the released queue contracts, including raw retry text
//! and presence-only markers. JSON syntax alone does not validate a queued task.
use std::{io, path::Path};

fn json<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> io::Result<()> {
    serde_json::from_slice::<T>(bytes).map(|_| ()).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}
pub(super) fn validate(family: &str, name: &str, bytes: &[u8]) -> io::Result<()> {
    let queue = family.rsplit('/').next().unwrap_or("");
    if !matches!(
        queue,
        "received"
            | "received-controls"
            | "job-deliveries"
            | "on-deliveries"
            | "on-stops"
            | "run-stops"
            | "run-requests"
    ) {
        return Ok(());
    }
    let extension = Path::new(name).extension().and_then(|extension| extension.to_str()).unwrap_or("");
    match extension {
        "json" => match queue {
            "received" => json::<crate::received::Entry>(bytes),
            "received-controls" => json::<crate::control_letters::Entry>(bytes),
            "job-deliveries" => crate::job_receipts::validate_legacy_sending(bytes),
            "on-deliveries" => json::<crate::on_delivery::Parked>(bytes),
            "on-stops" => crate::on_stop::validate_legacy_pending(bytes),
            "run-stops" => json::<crate::run_stops::Pending>(bytes),
            "run-requests" => json::<crate::run_stops::Incoming>(bytes),
            _ => unreachable!("known queue"),
        },
        "retry" => {
            std::str::from_utf8(bytes).map(|_| ()).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
        }
        "done" => json::<crate::obligations::Finished>(bytes),
        "posted" => json::<crate::control_posts::Posted>(bytes),
        "reply" => json::<crate::control_posts::Reply>(bytes),
        "answer" if queue == "run-stops" => json::<crate::run_stops::Answered>(bytes),
        "stopped" if queue == "run-requests" => json::<fastctx_types::access::HubAccount>(bytes),
        "" if name == "sequence" && matches!(queue, "received" | "received-controls") => std::str::from_utf8(bytes)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?
            .parse::<u64>()
            .map(|_| ())
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error)),
        _ => Ok(()),
    }
}
