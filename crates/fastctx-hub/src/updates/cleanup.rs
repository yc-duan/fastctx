//! A try's temporary package file goes once nothing uses it.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::Hub;

/// How many temporary files may be left to remove in the background. Past
/// this one more is left where it is for the next start to clear
/// (`fastctx_update::Cache::clear_attempts`), so that files that will not go
/// neither pile up nor hold up the package after them.
const BEHIND_AT_MOST: usize = 8;

/// How long one file is tried again before it is left for the next start.
const TRYING_AT_MOST: Duration = Duration::from_secs(10 * 60);

static BEHIND: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Removes the temporary file `path` of one try at a package, in the
/// background, tried again a while: a file the system keeps open a while,
/// as an antivirus scanner does, goes later. Its try's end and the next
/// package never wait for it. The file's name is that try's alone, so no
/// other try reuses it meanwhile.
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
        if let Err(error) = tokio::fs::remove_file(&path).await
            && error.kind() != std::io::ErrorKind::NotFound
        {
            hub.log.line(format_args!(
                "removing temporary package {}: {error}; {BEHIND_AT_MOST} others are still being tried, so it is left for the hub's next start",
                path.display()
            ));
        }
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
    let until = tokio::time::Instant::now() + TRYING_AT_MOST;
    let mut previous = None;
    let mut delay = Duration::from_secs(1);
    loop {
        match tokio::fs::remove_file(path).await {
            Ok(()) => return,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
            Err(error) => {
                let reason = error.to_string();
                if tokio::time::Instant::now() >= until {
                    hub.log.line(format_args!(
                        "removing temporary package {}: {reason}; left for the hub's next start",
                        path.display()
                    ));
                    return;
                }
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
