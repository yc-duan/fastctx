//! Retiring legacy file names waits for recovery copying, without delaying service.
use crate::store::{migrate, shelf::Shelf};
use std::{io, path::Path, sync::Arc};

pub(in crate::store) fn finish_legacy(shelf: &Shelf, dir: &Path, log: Option<Arc<crate::log::Log>>) -> io::Result<()> {
    let Some(gate) = shelf.0.format_upgrade_gate().filter(|gate| !gate.finished()) else {
        return migrate::back_up(shelf, dir);
    };
    let (shelf, dir) = (shelf.clone(), dir.to_path_buf());
    std::thread::Builder::new().name("hub-upgrade-followup".into()).spawn(move || {
        let report = |message: String| match &log {
            Some(log) => log.line(&message),
            None => eprintln!("{message}"),
        };
        if let Err(error) = gate.wait_blocking() {
            report(format!("upgrade backup ended without a durable result: {error}; continuing legacy migration"));
        }
        if let Err(error) = migrate::back_up(&shelf, &dir) {
            report(format!("retiring legacy hub file names failed: {error}; sources retained for restart"));
        }
    })?;
    Ok(())
}
