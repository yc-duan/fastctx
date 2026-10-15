//! The packages a machine holds: `v<version>/<target>`, each put in place
//! only once its signature checked out, with the manifest of its release
//! beside it. A package still coming is `<target>.part`.

use std::path::{Path, PathBuf};

use fastctx_types::update::{Held, Kept, MANIFEST, Manifest, target_name};

#[derive(Clone, Debug)]
pub struct Cache {
    root: PathBuf,
}

impl Cache {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn version_dir(&self, version: &str) -> Result<PathBuf, String> {
        if fastctx_types::update::release(version).is_none() {
            return Err(format!("{version} is not a release's version"));
        }
        Ok(self.root.join(format!("v{version}")))
    }

    /// Where the package `target` of `version` is kept.
    pub fn path(&self, version: &str, target: &str) -> Result<PathBuf, String> {
        if !target_name(target) || target == MANIFEST {
            return Err(format!("{target} is not a package's name"));
        }
        Ok(self.version_dir(version)?.join(target))
    }

    /// The package, when it is here.
    pub fn get(&self, version: &str, target: &str) -> Option<PathBuf> {
        self.path(version, target).ok().filter(|path| path.is_file())
    }

    /// Puts `file`, checked, in place as the package `target` of `version`.
    pub fn keep(&self, version: &str, target: &str, file: &Path) -> Result<PathBuf, String> {
        let path = self.path(version, target)?;
        std::fs::rename(file, &path).map_err(|error| format!("keeping {}: {error}", path.display()))?;
        Ok(path)
    }

    /// Where a package comes to before it is checked; its directory made.
    pub fn incoming(&self, version: &str, target: &str) -> Result<PathBuf, String> {
        let path = self.path(version, target)?;
        let dir = path.parent().expect("a package has a directory");
        std::fs::create_dir_all(dir).map_err(|error| format!("{}: {error}", dir.display()))?;
        Ok(fastctx_net::link::versions::partial(&path))
    }

    /// Keeps the manifest of a release whose package is kept here.
    pub fn keep_manifest(&self, manifest: &Manifest) -> Result<(), String> {
        let dir = self.version_dir(&manifest.version)?;
        std::fs::create_dir_all(&dir).map_err(|error| format!("{}: {error}", dir.display()))?;
        let text = serde_json::to_vec_pretty(manifest).map_err(|error| error.to_string())?;
        let path = dir.join(MANIFEST);
        let temporary = dir.join(format!("{MANIFEST}.part"));
        std::fs::write(&temporary, text).map_err(|error| format!("{}: {error}", temporary.display()))?;
        std::fs::rename(&temporary, &path).map_err(|error| format!("{}: {error}", path.display()))
    }

    /// The manifests kept, newest first.
    pub fn manifests(&self) -> Vec<Manifest> {
        let mut manifests: Vec<Manifest> = self
            .versions()
            .into_iter()
            .filter_map(|(_, dir)| std::fs::read(dir.join(MANIFEST)).ok())
            .filter_map(|bytes| serde_json::from_slice::<Manifest>(&bytes).ok())
            .collect();
        manifests.sort_by_key(|manifest| std::cmp::Reverse(manifest.semver()));
        manifests
    }

    /// The packages held, checked.
    pub fn held(&self) -> Vec<Held> {
        self.kept().into_iter().map(|kept| Held { version: kept.version, target: kept.target }).collect()
    }

    /// The packages held, checked, with where each is.
    pub fn kept(&self) -> Vec<Kept> {
        let mut kept = Vec::new();
        for (version, dir) in self.versions() {
            let Ok(entries) = std::fs::read_dir(&dir) else { continue };
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name == MANIFEST || name.ends_with(".part") || !target_name(&name) {
                    continue;
                }
                if entry.file_type().is_ok_and(|kind| kind.is_file()) {
                    kept.push(Kept {
                        version: version.clone(),
                        target: name,
                        path: entry.path().display().to_string(),
                    });
                }
            }
        }
        kept.sort_by(|a, b| (&a.version, &a.target).cmp(&(&b.version, &b.target)));
        kept
    }

    /// Removes every version but those in `keep`.
    pub fn prune(&self, keep: &[&str]) {
        for (version, dir) in self.versions() {
            if !keep.contains(&version.as_str()) {
                let _ = std::fs::remove_dir_all(dir);
            }
        }
    }

    fn versions(&self) -> Vec<(String, PathBuf)> {
        let Ok(entries) = std::fs::read_dir(&self.root) else { return Vec::new() };
        entries
            .flatten()
            .filter_map(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                let version = name.strip_prefix('v')?.to_string();
                fastctx_types::update::release(&version)?;
                entry.file_type().ok()?.is_dir().then(|| (version, entry.path()))
            })
            .collect()
    }
}
