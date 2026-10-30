//! Shared folder publication for account-owned resources. Relative names are
//! validated before touching disk; cleanup never follows a directory link.
use fastctx_types::skill_files::File;
use std::{collections::BTreeMap, io, path::Path};

pub(crate) fn erase(path: &Path) -> io::Result<()> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if metadata.file_type().is_symlink() {
        return if path.is_dir() { std::fs::remove_dir(path) } else { std::fs::remove_file(path) };
    }
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            erase(&entry.path())?;
        } else {
            #[cfg(windows)]
            {
                let mut permissions = entry.metadata()?.permissions();
                #[expect(
                    clippy::permissions_set_readonly_false,
                    reason = "Windows-only: clear the DOS read-only attribute before deletion"
                )]
                permissions.set_readonly(false);
                std::fs::set_permissions(entry.path(), permissions)?;
            }
            std::fs::remove_file(entry.path())?;
        }
    }
    std::fs::remove_dir(path)
}

pub(crate) fn unpack(directory: &Path, files: &[File]) -> io::Result<()> {
    fastctx_types::skill_files::validate_files(files).map_err(io::Error::other)?;
    fastctx_platform::ensure_private_dir(directory)?;
    let directory = std::fs::canonicalize(directory)?;
    for file in files {
        let path = directory.join(&file.path);
        let Some(bytes) = file.bytes().map_err(io::Error::other)? else {
            fastctx_platform::ensure_private_dir(&path)?;
            continue;
        };
        fastctx_platform::ensure_private_dir(path.parent().expect("file parent"))?;
        fastctx_platform::publish(&path, &bytes, fastctx_platform::Permissions::Private)?;
        let mut permissions = std::fs::metadata(&path)?.permissions();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            permissions.set_mode(if file.executable { 0o500 } else { 0o400 });
        }
        #[cfg(windows)]
        permissions.set_readonly(true);
        std::fs::set_permissions(path, permissions)?;
    }
    Ok(())
}

pub(crate) fn complete(directory: &Path, files: &[File]) -> io::Result<bool> {
    let mut expected = BTreeMap::new();
    for file in files {
        expected.insert(file.path.clone(), file.data.is_some());
        let mut path = file.path.as_str();
        while let Some((parent, _)) = path.rsplit_once('/') {
            expected.entry(parent.into()).or_insert(false);
            path = parent;
        }
    }
    fn walk(root: &Path, path: &Path, actual: &mut BTreeMap<String, bool>) -> io::Result<bool> {
        let metadata = match std::fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error),
        };
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Ok(false);
        }
        for entry in std::fs::read_dir(path)? {
            let entry = entry?;
            let path = entry.path();
            let kind = entry.file_type()?;
            if kind.is_symlink() {
                return Ok(false);
            }
            let key = path
                .strip_prefix(root)
                .expect("walked child")
                .components()
                .map(|part| part.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            actual.insert(key, kind.is_file());
            if kind.is_dir() && !walk(root, &path, actual)? {
                return Ok(false);
            }
        }
        Ok(true)
    }
    let mut actual = BTreeMap::new();
    if !walk(directory, directory, &mut actual)? || expected != actual {
        return Ok(false);
    }
    for file in files {
        if let Some(bytes) = file.bytes().map_err(io::Error::other)? {
            let path = directory.join(&file.path);
            if std::fs::read(&path)? != bytes {
                return Ok(false);
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                if (std::fs::metadata(path)?.permissions().mode() & 0o111 != 0) != file.executable {
                    return Ok(false);
                }
            }
        }
    }
    Ok(true)
}
