//! A package owner retains its place until its temporary file is gone.

use std::path::Path;
use std::time::Duration;

use crate::Hub;

pub(super) async fn temporary(hub: &Hub, path: &Path) {
    let mut previous = None;
    let mut delay = Duration::from_secs(1);
    loop {
        match tokio::fs::remove_file(path).await {
            Ok(()) => return,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
            Err(error) => {
                let reason = error.to_string();
                if previous.as_ref() != Some(&reason) {
                    hub.log.line(format_args!(
                        "removing temporary package {}: {reason}; its owner is still finishing",
                        path.display()
                    ));
                    previous = Some(reason);
                }
            }
        }
        tokio::time::sleep(delay).await;
        delay = (delay * 2).min(Duration::from_secs(30));
    }
}
