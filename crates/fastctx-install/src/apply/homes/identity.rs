//! Directory identity follows the filesystem, including case-sensitive Windows directories.

use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Key {
    Existing(crate::disk::Identity),
    Missing(PathBuf),
}

pub(super) fn resolve(home: &Path) -> Result<PathBuf, Error> {
    if home.as_os_str().is_empty() {
        return Err(Error::Io("host home must not be empty".into()));
    }
    match std::fs::canonicalize(home) {
        Ok(path) if path.is_dir() => Ok(path),
        Ok(_) => Err(Error::Io(format!("{} is not a home directory", home.display()))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let (path, _) = crate::snapshot::resolve(&home.join(".fastctx-home"))
                .map_err(|error| FileError::io(home, "resolve host home", error))?;
            Ok(path.parent().expect("resolved file has a parent").to_owned())
        }
        Err(error) => Err(FileError::io(home, "resolve host home", error).into()),
    }
}

pub(super) fn key(home: &Path) -> Result<Key, Error> {
    let path = resolve(home)?;
    match crate::disk::open_directory(&path).and_then(|file| crate::disk::identity(&file)) {
        Ok(identity) => Ok(Key::Existing(identity)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Key::Missing(path)),
        Err(error) => Err(FileError::io(home, "identify host home", error).into()),
    }
}

pub(super) fn spelling(home: &Path) -> Result<String, Error> {
    let home = resolve(home)?;
    let text = home.to_str().ok_or_else(|| Error::Io("host home must be a UTF-8 path".into()))?;
    Ok(if cfg!(windows) { text.replace('\\', "/").trim_start_matches("//?/").to_owned() } else { text.to_owned() })
}
