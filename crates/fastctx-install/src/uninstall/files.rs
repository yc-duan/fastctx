use std::path::{Path, PathBuf};
use std::{fs, io};

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
                {
                    let mut permissions = metadata.permissions();
                    // Windows has only the read-only attribute to clear.
                    #[allow(clippy::permissions_set_readonly_false)]
                    permissions.set_readonly(false);
                    fs::set_permissions(entry.path(), permissions)?;
                }
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let mut permissions = metadata.permissions();
                    permissions.set_mode(permissions.mode() | 0o200);
                    fs::set_permissions(entry.path(), permissions)?;
                }
            }
            delete_file(&entry.path())?;
        }
    }
    fs::remove_dir(path)
}

/// What retiring the program directories left: additions that are not
/// fastctx's, and owned files that could not be removed, each with why.
pub struct Programs {
    pub left: Vec<PathBuf>,
    pub failed: Vec<(PathBuf, io::Error)>,
}

/// Retires executable assets while leaving arbitrary additions to bin intact.
/// Every owned file is tried; one that cannot go does not keep the rest.
pub fn remove_programs(root: &Path) -> Programs {
    let mut programs = Programs { left: Vec::new(), failed: Vec::new() };
    for bin in [root.join("bin"), root.join("world/bin"), root.join("renewal/images")] {
        if let Err(error) = retire_bin(&bin, &mut programs) {
            programs.failed.push((bin, error));
        }
    }
    programs
}

fn retire_bin(bin: &Path, programs: &mut Programs) -> io::Result<()> {
    super::checked_path(bin)?;
    match fs::symlink_metadata(bin) {
        Ok(metadata) if crate::disk::is_link(&metadata) => {
            programs.left.push(bin.to_path_buf());
            return Ok(());
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
        _ => {}
    }
    let entries = match fs::read_dir(bin) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let path = entry?.path();
        if let Err(error) = retire_entry(&path, programs) {
            programs.failed.push((path, error));
        }
    }
    if fs::read_dir(bin)?.next().is_none() {
        fs::remove_dir(bin)?;
    }
    Ok(())
}

fn retire_entry(path: &Path, programs: &mut Programs) -> io::Result<()> {
    let name = path.file_name().and_then(|name| name.to_str()).unwrap_or("");
    let metadata = fs::symlink_metadata(path)?;
    if metadata.is_dir()
        && !crate::disk::is_link(&metadata)
        && matches!(name.len(), 24 | 64)
        && name.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        retire_bin(path, programs)
    } else if name.starts_with(".fastctx-exe-retired-") {
        crate::executable::delete_retired(path).map_err(|error| io::Error::other(error.to_string()))
    } else if !metadata.is_dir()
        && matches!(
            name,
            "fastctx"
                | "fastctx.exe"
                | "rg"
                | "rg.exe"
                | "prelude.bash"
                | "LICENSE-APACHE"
                | "NOTICE"
                | "THIRD_PARTY_LICENSES.md"
                | "fastctx.renewal-files.json"
        )
    {
        delete_file(path)
    } else {
        programs.left.push(path.to_path_buf());
        Ok(())
    }
}
