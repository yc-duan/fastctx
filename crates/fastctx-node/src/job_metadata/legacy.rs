//! Scalar sidecars have distinct released formats. Presence markers are not
//! JSON, and the older pending-facts form remains accepted by its real reader.
use std::io;
fn json<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> io::Result<()> {
    serde_json::from_slice::<T>(bytes).map(|_| ()).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}
fn text(bytes: &[u8]) -> io::Result<&str> {
    std::str::from_utf8(bytes).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}
pub(super) fn validate(kind: &str, bytes: &[u8]) -> io::Result<()> {
    match kind {
        "owed" if bytes.is_empty() || bytes == b"answer" => Ok(()),
        "owed" => Err(io::Error::new(io::ErrorKind::InvalidData, "unknown completion debt form")),
        "notice" => json::<fastctx_types::residents::Notice>(bytes),
        "delivered" => json::<fastctx_types::run::JobDelivery>(bytes),
        "wait-taken" => {
            let value = text(bytes)?;
            if value.is_empty() {
                Ok(())
            } else {
                value
                    .parse::<fastctx_types::NoticeId>()
                    .map(|_| ())
                    .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
            }
        }
        "on-stopped" => json::<fastctx_types::run::StopRequest>(bytes),
        "answer-wait" => json::<crate::completion_debt::Waiting>(bytes),
        "posted" => json::<crate::letters::Posted>(bytes),
        "facts-state" => json::<crate::run_facts::updates::Emitted>(bytes),
        "facts-pending" => json::<crate::run_facts::updates::Recovery>(bytes),
        "facts-output" => json::<crate::run_facts::output::Measured>(bytes),
        "done" => json::<crate::obligations::Finished>(bytes),
        "retry" => text(bytes).map(|_| ()),
        "exit-frozen" => json::<bool>(bytes),
        _ => Ok(()),
    }
}
