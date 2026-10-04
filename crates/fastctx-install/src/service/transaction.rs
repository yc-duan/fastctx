//! An interrupted registration is restored before the next mutation. The
//! journal is removed only after the manager and its supporting files agree.

use std::path::Path;

use anyhow::{Context as _, bail};
use fastctx_platform::atomic::{Permissions, publish_new};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

#[derive(Serialize, Deserialize)]
struct Journal<T> {
    owner: String,
    before: T,
}

pub(super) fn run<T: Serialize + DeserializeOwned, R>(
    path: &Path,
    owner: &str,
    capture: impl FnOnce() -> anyhow::Result<T>,
    change: impl FnOnce(&T) -> anyhow::Result<R>,
    restore: impl Fn(&T) -> anyhow::Result<()>,
) -> anyhow::Result<R> {
    match std::fs::read(path) {
        Ok(bytes) => {
            let journal: Journal<T> =
                serde_json::from_slice(&bytes).context("reading interrupted service transaction")?;
            if journal.owner != owner {
                bail!("{} belongs to another service", path.display());
            }
            restore(&journal.before).context("restoring an interrupted service transaction")?;
            std::fs::remove_file(path)?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let journal = Journal { owner: owner.into(), before: capture()? };
    publish_new(path, &serde_json::to_vec(&journal)?, Permissions::Private)?;
    let result = change(&journal.before).and_then(|value| {
        std::fs::remove_file(path).context("committing the service transaction")?;
        Ok(value)
    });
    match result {
        Ok(value) => Ok(value),
        Err(error) => match restore(&journal.before).and_then(|()| std::fs::remove_file(path).map_err(Into::into)) {
            Ok(()) => Err(error.context("service operation failed; previous registration restored")),
            Err(rollback) => Err(error.context(format!(
                "service operation failed; rollback also failed: {rollback:#}; recovery record: {}",
                path.display()
            ))),
        },
    }
}

pub(super) fn require_complete(path: &Path) -> anyhow::Result<()> {
    if path.try_exists()? {
        bail!(
            "a service operation is in progress or was interrupted ({}); run install or uninstall to finish it",
            path.display()
        );
    }
    Ok(())
}
