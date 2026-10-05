use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Stamp(Option<(u64, Option<SystemTime>)>);

impl Stamp {
    pub fn is_directory(&self) -> bool {
        matches!(self.0, Some((_, None)))
    }
}

pub(crate) fn stamp(path: &Path) -> io::Result<Stamp> {
    match fs::symlink_metadata(path) {
        Ok(meta) if crate::paths::linked(&meta) || (!meta.is_file() && !meta.is_dir()) => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("ignore policy is not a regular file or directory: {}", path.display()),
        )),
        Ok(meta) => Ok(Stamp(Some(if meta.is_dir() { (0, None) } else { (meta.len(), Some(meta.modified()?)) }))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Stamp(None)),
        Err(error) => Err(error),
    }
}

pub(crate) fn directory(path: &Path, files: &mut BTreeMap<PathBuf, Stamp>) -> io::Result<()> {
    for name in [".ignore", ".gitignore", ".git"] {
        let file = path.join(name);
        files.insert(file.clone(), stamp(&file)?);
    }
    let git = path.join(".git");
    let git = if git.is_file() {
        let contents = fs::read_to_string(&git)?;
        let Some(directory) = contents.trim().strip_prefix("gitdir: ") else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("invalid git directory file: {}", git.display()),
            ));
        };
        let git = path.join(directory);
        let common = git.join("commondir");
        files.insert(common.clone(), stamp(&common)?);
        match fs::read_to_string(&common) {
            Ok(directory) => git.join(directory.trim()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => git,
            Err(error) => return Err(error),
        }
    } else {
        git
    };
    let info = git.join("info");
    files.insert(info.clone(), stamp(&info)?);
    let exclude = info.join("exclude");
    files.insert(exclude.clone(), stamp(&exclude)?);
    Ok(())
}

pub(crate) fn global() -> io::Result<BTreeMap<PathBuf, Stamp>> {
    let mut paths = Vec::new();
    if std::env::var_os("GIT_CONFIG_SYSTEM").is_none_or(|path| path.is_empty()) {
        paths.push(std::path::absolute("/etc/gitconfig")?);
    }
    for var in ["GIT_CONFIG_GLOBAL", "GIT_CONFIG_SYSTEM"] {
        if let Some(path) = std::env::var_os(var).filter(|path| !path.is_empty()) {
            paths.push(PathBuf::from(path));
        }
    }
    // Use the same home lookup as ignore, including its platform behavior.
    #[allow(deprecated)]
    let home = std::env::home_dir();
    if let Some(home) = home {
        paths.push(home.join(".gitconfig"));
        paths.push(home.join(".config/git/config"));
    }
    if let Some(path) = std::env::var_os("XDG_CONFIG_HOME") {
        paths.push(PathBuf::from(path).join("git/config"));
    }
    let mut files =
        paths.into_iter().map(|path| Ok((path.clone(), stamp(&path)?))).collect::<io::Result<BTreeMap<_, _>>>()?;
    if let Some(path) = ignore::gitignore::gitconfig_excludes_path() {
        files.insert(path.clone(), stamp(&path)?);
    }
    Ok(files)
}

pub(crate) fn changed(files: &BTreeMap<PathBuf, Stamp>) -> io::Result<bool> {
    for (path, old) in files {
        if stamp(path)? != *old {
            return Ok(true);
        }
    }
    Ok(false)
}
