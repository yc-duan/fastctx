//! Last reports are keyed by identity and written by one coalescing writer.

use fastctx_net::sign::PublicKey;
use fastctx_types::update::Report;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;
use tokio::sync::watch;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct LastReport {
    pub at: Timestamp,
    pub report: Report,
}

pub(super) type History = BTreeMap<PublicKey, LastReport>;

pub(super) fn open(dir: &Path) -> Result<(History, watch::Sender<History>), String> {
    let path = dir.join("reports.json");
    let last = match std::fs::read(&path) {
        Ok(bytes) => match serde_json::from_slice(&bytes) {
            Ok(last) => last,
            Err(error) => return Err(format!("reading {}: {error}", path.display())),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => History::new(),
        Err(error) => return Err(format!("reading {}: {error}", path.display())),
    };
    let (writer, mut receiver) = watch::channel(last.clone());
    tokio::spawn(async move {
        while receiver.changed().await.is_ok() {
            let mut delay = std::time::Duration::from_secs(1);
            loop {
                let snapshot = receiver.borrow_and_update().clone();
                let path = path.clone();
                let result = tokio::task::spawn_blocking(move || -> Result<(), String> {
                    std::fs::create_dir_all(path.parent().expect("reports have a directory"))
                        .map_err(|e| e.to_string())?;
                    let bytes = serde_json::to_vec(&snapshot).map_err(|e| e.to_string())?;
                    fastctx_platform::publish(&path, &bytes, fastctx_platform::Permissions::Private)
                        .map_err(|e| e.to_string())
                })
                .await;
                if matches!(result, Ok(Ok(()))) {
                    break;
                }
                eprintln!("keeping update reports: {result:?}");
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(std::time::Duration::from_secs(60));
            }
        }
    });
    Ok((last, writer))
}
