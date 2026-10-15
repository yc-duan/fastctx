//! Read-only coordinates and file operations for uninstalling one local World.

mod application;
mod files;
mod legacy;
mod path;
pub use path::ZPROFILE_LINES;

pub use application::{application_directories, remove_application_data};
pub use files::{Programs, delete_file, delete_tree, remove_programs};
pub use legacy::{Legacy, legacy};
pub use path::{path_entry, remove_path};

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Coordinates captured under the original user's identity. Elevation never
/// resolves the administrator's home or host directories in their place.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Installation {
    pub root: PathBuf,
    pub home: PathBuf,
    pub codex_home: PathBuf,
    pub claude_home: PathBuf,
    pub claude_override: Option<PathBuf>,
    pub application_dirs: Vec<PathBuf>,
    pub application: Option<PathBuf>,
    #[serde(default)]
    pub application_image: Option<PathBuf>,
    #[serde(default)]
    pub application_process: Option<fastctx_types::ProcessIdentity>,
    #[serde(default)]
    pub application_control: Option<PathBuf>,
    #[serde(default)]
    pub graphical: bool,
    #[cfg(windows)]
    pub user_sid: String,
    #[cfg(unix)]
    pub uid: u32,
}

impl Installation {
    pub fn here() -> std::io::Result<Self> {
        let places = crate::apply::Places::here()?;
        let home = fastctx_platform::user_home()?;
        let application_dirs = if places.root == home.join(".fastctx") {
            application_directories(&home)?
                .into_iter()
                .filter_map(|path| match path.try_exists() {
                    Ok(true) => Some(Ok(path)),
                    Ok(false) => None,
                    Err(error) => Some(Err(error)),
                })
                .collect::<std::io::Result<_>>()?
        } else {
            Vec::new()
        };
        Ok(Self {
            root: places.root,
            home,
            codex_home: places.codex_home,
            claude_home: places.claude_home,
            claude_override: std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from),
            application_dirs,
            application: None,
            application_image: None,
            application_process: None,
            application_control: None,
            graphical: false,
            #[cfg(windows)]
            user_sid: fastctx_platform::current_user_sid()?,
            #[cfg(unix)]
            uid: unsafe { libc::getuid() },
        })
    }

    pub fn places(&self, executable: PathBuf) -> crate::apply::Places {
        crate::apply::Places {
            root: self.root.clone(),
            world: self.root.join("world"),
            codex_home: self.codex_home.clone(),
            claude_home: self.claude_home.clone(),
            claude: crate::claude::find(std::env::var_os("PATH").as_ref()),
            fastctx: executable,
        }
    }

    pub fn marker(&self) -> PathBuf {
        self.root.join("uninstalled.json")
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(self.root.is_absolute() && self.home.is_absolute(), "uninstall paths must be absolute");
        anyhow::ensure!(
            self.root != self.home && self.root.parent().is_some(),
            "the user's home cannot be uninstalled"
        );
        checked_path(&self.root)?;
        match std::fs::symlink_metadata(&self.root) {
            Ok(metadata) => anyhow::ensure!(
                !crate::disk::is_link(&metadata),
                "the World root is redirected; its target cannot be treated as owned installation data"
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        if self.root.try_exists()? && self.home.try_exists()? {
            let root = std::fs::canonicalize(&self.root)?;
            let home = std::fs::canonicalize(&self.home)?;
            anyhow::ensure!(
                !home.starts_with(root),
                "the World root contains the user's home; it cannot be uninstalled"
            );
        }
        for directory in &self.application_dirs {
            anyhow::ensure!(
                directory.is_absolute() && application::is_application_directory(directory),
                "uninstall application data must name the World application directory"
            );
            checked_path(directory)?;
        }
        Ok(())
    }
}

pub(super) fn checked_path(path: &std::path::Path) -> std::io::Result<()> {
    use std::path::{Component, PathBuf};
    if !path.is_absolute() || path.components().any(|part| part == Component::ParentDir) {
        return Err(std::io::Error::other("uninstall paths must be absolute without parent traversal"));
    }
    let mut parent = PathBuf::new();
    for component in path.parent().unwrap_or(path).components() {
        parent.push(component);
        if matches!(component, Component::Prefix(_)) {
            continue;
        }
        match std::fs::symlink_metadata(&parent) {
            Ok(metadata) if crate::disk::is_link(&metadata) => {
                return Err(std::io::Error::other("an uninstall parent is redirected"));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => return Err(error),
        }
    }
    Ok(())
}
