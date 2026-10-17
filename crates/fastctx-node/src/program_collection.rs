//! Old per-node programs retire after their processes and reserved jobs do.

use std::fs;

use crate::Node;

pub(crate) fn collect(node: &Node) -> anyhow::Result<()> {
    let _collecting = node.disk.lock();
    collect_legacy(&node.world)?;
    fastctx_install::program_collection::collect(&node.root)?;
    let Some(_apply) = fastctx_platform::FileLock::try_acquire(&node.root.join("apply.lock"))? else { return Ok(()) };
    for error in fastctx_install::executable::cleanup_retired_unused(&fastctx_install::renewal::installed(&node.root)) {
        node.log.line(format_args!("collecting retired program images: {error}"));
    }
    fastctx_install::transaction::collect_finished(&node.root)?;
    Ok(())
}

pub(super) fn collect_legacy(world: &fastctx_types::layout::WorldDir) -> anyhow::Result<()> {
    let uses = fastctx_hands::run::program::uses(world)?;
    for directory in crate::retention::entries(&world.path().join("bin"))? {
        let metadata = fs::symlink_metadata(&directory)?;
        if fastctx_platform::owned_directory::is_link(&metadata) || !metadata.is_dir() {
            continue;
        }
        let Some(name) = directory.file_name().and_then(|name| name.to_str()) else { continue };
        if name.len() != 24 || !name.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            continue;
        }
        if uses
            .iter()
            .map(|usage| usage.reason_for(&directory))
            .collect::<std::io::Result<Vec<_>>>()?
            .iter()
            .any(Option::is_some)
        {
            continue;
        }
        let image = directory.join(fastctx_install::renewal::EXECUTABLE);
        let _unused = if image.try_exists()? {
            let Some(unused) = fastctx_platform::image_usage::unused(&image)? else { continue };
            Some(unused)
        } else {
            None
        };
        for file in crate::retention::entries(&directory)? {
            let owned = file.file_name().is_some_and(|name| {
                [
                    "fastctx",
                    "fastctx.exe",
                    "rg",
                    "rg.exe",
                    "prelude.bash",
                    "LICENSE-APACHE",
                    "NOTICE",
                    "THIRD_PARTY_LICENSES.md",
                ]
                .iter()
                .any(|owned| name == *owned)
            });
            if owned && fs::symlink_metadata(&file)?.is_file() {
                fs::remove_file(file)?;
            }
        }
        drop(_unused);
        if crate::retention::entries(&directory)?.is_empty() {
            fs::remove_dir(directory)?;
        }
    }
    Ok(())
}
