//! Concurrent first users share one initial placement before entering the
//! stable kernel. Explicit upgrades retain their existing conflict behavior.

use std::path::Path;

pub(crate) fn ensure(root: &Path, source: &Path) -> anyhow::Result<()> {
    let installed = fastctx_install::executable::installed_path(root);
    if installed.is_file() && fastctx_install::program::inspect(&installed)?.protocol != 0 {
        return Ok(());
    }
    let directory = fastctx_install::renewal::directory(root);
    fastctx_platform::ensure_private_dir(&directory)?;
    let _first = fastctx_platform::FileLock::acquire(&directory.join("first-placement.lock"))?;
    // The winner publishes before releasing this lock. Recheck instead of
    // making the next session stage that same first program again.
    if !installed.is_file() || fastctx_install::program::inspect(&installed)?.protocol == 0 {
        fastctx_install::placement::ensure(root, source)?;
    }
    Ok(())
}
