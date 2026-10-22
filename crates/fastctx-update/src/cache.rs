//! The packages a machine holds: `v<version>/<target>`, each put in place
//! only once its signature checked out, with the manifest of its release
//! beside it. A package still coming is `<target>.part`; one a hub brings in
//! by several tries in turn, `<target>.<try>.part` for each.

use std::path::{Path, PathBuf};

use fastctx_types::update::{Held, Kept, MANIFEST, Manifest, target_name};

/// What ends the name of a package one try is bringing in (`Cache::attempt`).
const ATTEMPT: &str = ".part";

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

    /// Where the try numbered `attempt` of bringing a package in writes it
    /// before it is checked; its directory made. Each try has its own, so
    /// two tries never write, cut or delete one file.
    pub fn attempt(&self, version: &str, target: &str, attempt: u64) -> Result<PathBuf, String> {
        let path = self.path(version, target)?;
        let dir = path.parent().expect("a package has a directory");
        std::fs::create_dir_all(dir).map_err(|error| format!("{}: {error}", dir.display()))?;
        Ok(dir.join(format!("{target}.{attempt}{ATTEMPT}")))
    }

    /// Incomplete files for exactly this package. Callers must exclude its live
    /// acquisition owner before deleting any of these paths.
    pub fn incomplete(&self, version: &str, target: &str) -> Result<Vec<PathBuf>, String> {
        let path = self.path(version, target)?;
        let directory = path.parent().expect("package directory");
        let entries = match std::fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(format!("reading incomplete packages in {}: {error}", directory.display())),
        };
        let mut paths = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|e| format!("reading {}: {e}", directory.display()))?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.strip_prefix(&format!("{target}.")).and_then(|rest| rest.strip_suffix("part")).is_some_and(|rest| {
                rest.is_empty() || rest.strip_suffix('.').is_some_and(|number| number.parse::<u64>().is_ok())
            }) {
                paths.push(entry.path());
            }
        }
        Ok(paths)
    }

    /// Deletes what tries of bringing packages in left: none outlives the
    /// process that made it. Says what it could not delete.
    pub fn clear_attempts(&self) -> Vec<String> {
        let mut failed = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&self.root) {
            for entry in entries.flatten() {
                if entry.file_name().to_string_lossy().starts_with(".retired-")
                    && entry.file_type().is_ok_and(|kind| kind.is_dir())
                    && let Err(error) = std::fs::remove_dir_all(entry.path())
                {
                    failed.push(format!("{}: {error}", entry.path().display()));
                }
            }
        }
        for (_, dir) in self.versions() {
            let Ok(entries) = std::fs::read_dir(&dir) else { continue };
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                let left = name.strip_suffix(ATTEMPT).and_then(|rest| rest.rsplit_once('.'));
                if left.is_some_and(|(_, number)| number.parse::<u64>().is_ok())
                    && let Err(error) = std::fs::remove_file(entry.path())
                {
                    failed.push(format!("{}: {error}", entry.path().display()));
                }
            }
        }
        failed
    }

    /// Keeps the manifest of a release whose package is kept here.
    pub fn keep_manifest(&self, manifest: &Manifest) -> Result<(), String> {
        let dir = self.version_dir(&manifest.version)?;
        std::fs::create_dir_all(&dir).map_err(|error| format!("{}: {error}", dir.display()))?;
        let text = serde_json::to_vec_pretty(manifest).map_err(|error| error.to_string())?;
        let path = dir.join(MANIFEST);
        // Two packages of one release kept at once each write their own.
        fastctx_platform::publish(&path, &text, fastctx_platform::Permissions::Private)
            .map_err(|error| format!("{}: {error}", path.display()))
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
                // Older application installers left executable copies beside packages (2026-10-21).
                if name == MANIFEST || name.ends_with(".part") || name.ends_with(".exe") || !target_name(&name) {
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

    /// Moves an unused release out of the inventory in one filesystem step.
    /// Its returned directory can be deleted without blocking new acquisitions.
    pub fn retire(&self, version: &str) -> Result<Option<PathBuf>, String> {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let source = self.version_dir(version)?;
        let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let retired = self.root.join(format!(".retired-{}-{id}-{version}", std::process::id()));
        match std::fs::rename(&source, &retired) {
            Ok(()) => Ok(Some(retired)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(format!("retiring {}: {error}", source.display())),
        }
    }

    /// Removes every version but those in `keep`.
    pub fn prune(&self, keep: &[&str]) {
        for (version, dir) in self.versions() {
            if !keep.contains(&version.as_str()) {
                let _ = std::fs::remove_dir_all(dir);
            }
        }
    }

    /// Release directories currently present, excluding retired copies.
    pub fn versions(&self) -> Vec<(String, PathBuf)> {
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
