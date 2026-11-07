//! A visible publication is committed even if its durability check failed.

use std::{io, path::Path};

/// Returns a durability warning for an already-visible file, never a rejection.
pub(crate) fn publish(path: &Path, bytes: &[u8]) -> io::Result<Option<String>> {
    match fastctx_platform::publish_checked(path, bytes, fastctx_platform::Permissions::Private, true, || Ok(())) {
        Ok(()) => Ok(None),
        Err(failure) if failure.published => {
            Ok(Some(format!("{} is in place, but making it durable failed: {}", path.display(), failure.error)))
        }
        Err(failure) => Err(failure.error),
    }
}
