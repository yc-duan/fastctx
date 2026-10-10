use std::{fs, io};
use std::path::{Path, PathBuf};

/// Deletes one checked directory entry, including a mapped Windows image.
/// A retained hard link outside the installation lets a running CLI's old
/// image lose its installed name without deleting the executing object.
pub fn delete_file(path: &Path) -> io::Result<()> {
    super::checked_path(path)?;
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if crate::disk::is_link(&metadata) {
        return if metadata.is_dir() { fs::remove_dir(path) } else { fs::remove_file(path) };
    }
    let file = crate::disk::open(path)?;
    crate::disk::delete(&file, path)
}

/// Removes an explicitly selected product data tree without following links.
/// Mounted or redirected parents are rejected before walking descendants.
pub fn delete_tree(path: &Path) -> io::Result<()> {
    super::checked_path(path)?;
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if crate::disk::is_link(&metadata) || !metadata.is_dir() {
        return delete_file(path);
    }
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let metadata = fs::symlink_metadata(entry.path())?;
        if crate::disk::is_link(&metadata) {
            delete_file(&entry.path())?;
        } else if metadata.is_dir() {
            delete_tree(&entry.path())?;
        } else {
            if metadata.permissions().readonly() {
                #[cfg(windows)]
                { let mut permissions = metadata.permissions(); permissions.set_readonly(false); fs::set_permissions(entry.path(), permissions)?; }
                #[cfg(unix)]
                { use std::os::unix::fs::PermissionsExt; let mut permissions = metadata.permissions(); permissions.set_mode(permissions.mode() | 0o200); fs::set_permissions(entry.path(), permissions)?; }
            }
            delete_file(&entry.path())?;
        }
    }
    fs::remove_dir(path)
}

/// Retires executable assets while leaving arbitrary additions to bin intact.
pub fn remove_programs(root: &Path) -> io::Result<Vec<PathBuf>> {
    let mut left = Vec::new();
    for bin in [root.join("bin"), root.join("world/bin")] {
        retire_bin(&bin, &mut left)?;
    }
    let images = root.join("renewal/images");
    retire_bin(&images, &mut left)?;
    Ok(left)
}

fn retire_bin(bin: &Path, left: &mut Vec<PathBuf>) -> io::Result<()> {
    super::checked_path(bin)?;
    match fs::symlink_metadata(bin) {
        Ok(metadata) if crate::disk::is_link(&metadata) => { left.push(bin.to_path_buf()); return Ok(()); },
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
        _ => {},
    }
    let entries = match fs::read_dir(bin) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let path = entry?.path();
        let name = path.file_name().and_then(|name| name.to_str()).unwrap_or("");
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.is_dir() && !crate::disk::is_link(&metadata)
            && matches!(name.len(), 24 | 64) && name.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            retire_bin(&path, left)?;
        } else if name.starts_with(".fastctx-exe-retired-") {
            if let Err(error) = crate::executable::delete_retired(&path) {
                return Err(io::Error::other(error.to_string()));
            }
        } else if !metadata.is_dir() && (matches!(name, "fastctx" | "fastctx.exe" | "rg" | "rg.exe" | "prelude.bash"
            | "LICENSE-APACHE" | "NOTICE" | "THIRD_PARTY_LICENSES.md" | "fastctx.renewal-files.json")
            ) {
            delete_file(&path)?;
        } else {
            left.push(path);
        }
    }
    if fs::read_dir(bin)?.next().is_none() { fs::remove_dir(bin)?; }
    Ok(())
}
