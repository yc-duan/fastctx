//! The stable program supplies the node and everything it starts.

use std::path::{Path, PathBuf};

pub(crate) fn prepare(root: &Path) -> anyhow::Result<PathBuf> {
    let program = fastctx_install::executable::installed_path(root);
    anyhow::ensure!(
        std::fs::canonicalize(std::env::current_exe()?)? == std::fs::canonicalize(&program)?,
        "a node must run from the stable World program"
    );
    let directory = program.parent().expect("installed program has a bin directory");
    fastctx_ripgrep::place(directory)?;
    if cfg!(windows) {
        let shim = "#!/bin/sh\nMSYS_NO_PATHCONV=1 MSYS2_ARG_CONV_EXCL='*' exec \"${0%/*}/fastctx.exe\" \"$@\"\n";
        let path = directory.join("fastctx");
        if !std::fs::read(&path).is_ok_and(|bytes| bytes == shim.as_bytes()) {
            fastctx_platform::publish(&path, shim.as_bytes(), fastctx_platform::Permissions::Private)?;
        }
    }
    Ok(program)
}
