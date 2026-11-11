//! A try's temporary package file goes once nothing uses it.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::Hub;

/// How many temporary files may be left to remove in the background. Past
/// this the try that left one waits for it as it did before, so that files
/// that will not go do not pile up while package after package comes in.
const BEHIND_AT_MOST: usize = 8;

static BEHIND: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Removes the temporary file `path` of one try at a package, tried again
/// until it is gone: a file the system keeps open a while, as an antivirus
/// scanner does, goes later. It is removed in the background, so that it
/// holds up neither its try's end nor the next package, unless as many are
/// left behind already (`BEHIND_AT_MOST`). The file's name is that try's
/// alone, so no other try reuses it meanwhile.
pub(super) async fn temporary(hub: &Arc<Hub>, path: PathBuf) {
    use std::sync::atomic::Ordering;
    struct Behind;
    impl Drop for Behind {
        fn drop(&mut self) {
            BEHIND.fetch_sub(1, Ordering::AcqRel);
        }
    }
    let behind = Behind;
    if BEHIND.fetch_add(1, Ordering::AcqRel) >= BEHIND_AT_MOST {
        removed(hub.clone(), path).await;
        return;
    }
    let hub = hub.clone();
    fastctx_platform::task::spawn(async move {
        let _behind = behind;
        removed(hub, path).await;
    });
}

async fn removed(hub: Arc<Hub>, path: PathBuf) {
    let path = path.as_path();
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
                        "removing temporary package {}: {reason}; it is tried again",
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
