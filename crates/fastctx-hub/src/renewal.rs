//! A hub kept by a boot service on Linux or macOS runs straight from the
//! World program. The node that replaces that program restarts the hub where
//! it may; where it may not, a Mac's daemon or a Linux unit whose user has no
//! rule for it, the hub does so itself (`world-upgrade.md`, 中枢跟着换): it
//! looks at the program it was started from once a minute, and once that is a
//! release newer than its own, it stops as it would when told to, and its
//! service starts it again on the new one. Nobody is waited for.

/// How often the program the hub was started from is looked at.
#[cfg(unix)]
const LOOK: std::time::Duration = std::time::Duration::from_secs(60);

/// Resolves once the program this hub was started from has been replaced by
/// a newer release; never where there is no such program to look at, or no
/// service manager, the parent of a hub its service started, to start it
/// again.
pub(crate) async fn replaced() -> String {
    #[cfg(unix)]
    if std::os::unix::process::parent_id() == 1
        && let Ok(path) = std::env::current_exe()
    {
        let mut seen = shape(&path);
        loop {
            tokio::time::sleep(LOOK).await;
            let now = shape(&path);
            if now == seen {
                continue;
            }
            seen = now;
            let asked = path.clone();
            if let Ok(Some(version)) = tokio::task::spawn_blocking(move || newer(&asked)).await {
                return version;
            }
        }
    }
    std::future::pending().await
}

/// What tells a replaced file from the one there before without reading it.
#[cfg(unix)]
fn shape(path: &std::path::Path) -> Option<(u64, std::time::SystemTime)> {
    let metadata = std::fs::metadata(path).ok()?;
    Some((metadata.len(), metadata.modified().ok()?))
}

/// The release the program at `path` is, where it is newer than this hub's.
#[cfg(unix)]
fn newer(path: &std::path::Path) -> Option<String> {
    #[derive(serde::Deserialize)]
    struct Identity {
        version: String,
    }
    let output = std::process::Command::new(path)
        .args(["renew", "identify"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    let identity: Identity = serde_json::from_slice(&output.stdout).ok().filter(|_| output.status.success())?;
    fastctx_types::kernel::later(&identity.version, env!("CARGO_PKG_VERSION")).then_some(identity.version)
}
