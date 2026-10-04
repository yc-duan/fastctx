use std::env;
use std::path::{Path, PathBuf};

use super::{Error, Plan, archive, files, processes, updates};

pub(super) struct Paths {
    pub root: PathBuf,
    home: PathBuf,
    temp: PathBuf,
    runtime: Vec<PathBuf>,
    pdf: Vec<PathBuf>,
}

impl Paths {
    pub fn runtime_entry_count(&self) -> std::io::Result<usize> {
        self.runtime.iter().try_fold(0usize, |count, path| {
            count
                .checked_add(files::entries(path)?.len())
                .ok_or_else(|| std::io::Error::other("runtime entry count overflow"))
        })
    }

    pub fn legacy() -> Result<Self, Error> {
        let home = env::var_os("HOME")
            .or_else(|| env::var_os("USERPROFILE"))
            .map(PathBuf::from)
            .ok_or_else(|| Error::Io("legacy HOME and USERPROFILE are both unset".into()))?;
        files::check_path(&home).map_err(super::io)?;
        let temp = env::temp_dir();
        let mut runtime: Vec<_> = ["job-control", "edit-locks", "engine", "update-check"]
            .into_iter()
            .map(|component| runtime_dir(component, &temp))
            .collect();
        #[cfg(unix)]
        // SAFETY: geteuid has no preconditions. The released Unix socket
        // implementation uses this path when sockaddr_un cannot hold XDG's path.
        let fallback = Some(PathBuf::from(format!("/tmp/fastctx-engine-{}", unsafe { libc::geteuid() })));
        #[cfg(windows)]
        let fallback: Option<PathBuf> = None;
        if let Some(fallback) = fallback
            && !runtime.contains(&fallback)
        {
            runtime.push(fallback);
        }
        let primary = pdf_dir(&home);
        let identity = primary.as_ref().unwrap_or(&home);
        let fallback = temp.join(format!("fastctx-{}", &files::hash(identity.to_string_lossy().as_bytes())[..16]));
        let mut pdf: Vec<_> = primary.into_iter().collect();
        if !pdf.contains(&fallback) {
            pdf.push(fallback);
        }
        Ok(Self { root: home.join(".fastctx"), home, temp, runtime, pdf })
    }

    pub fn recovery_path(&self, path: &Path) -> bool {
        // Records can describe only the same narrow set of files this cleaner
        // itself writes. A changed record must never become arbitrary file IO.
        let parent = path.parent();
        let matches =
            |expected: &Path| std::fs::canonicalize(expected).ok().as_deref() == parent || Some(expected) == parent;
        let name = path.file_name().and_then(|name| name.to_str()).unwrap_or("");
        (matches(&self.root) && name == "config.toml")
            || (matches(&self.root.join("update"))
                && super::updates::nonce_pid(name).is_some()
                && ["helper-", "probe-", "health-"].iter().any(|prefix| name.starts_with(prefix)))
            || (self.pdf.iter().any(|directory| matches(directory)) && pdf_hash(name).is_some())
    }

    pub fn inspect_other(&self, plan: &mut Plan) {
        for path in super::entries(&self.root, plan) {
            let name = path.file_name().and_then(|name| name.to_str()).unwrap_or("");
            if name.starts_with("fastctx-npm-check-") || name.starts_with("fastctx-npm-preflight-") {
                self.cache(&path, plan);
            } else if let Some(writer) = super::leftovers::replacement_with_rf(name, "config.toml") {
                if writer.is_some_and(|pid| !plan.creator_ended(&path, pid)) {
                    continue;
                }
                if plan.quiet(&path, false) && plan.unused(&path, true) {
                    plan.archive(&path, "settings", "interrupted legacy settings write retained unchanged");
                }
            }
        }
        for name in [".fastread", ".fastshell", ".fastedit"] {
            let path = self.home.join(name);
            match std::fs::symlink_metadata(&path) {
                Ok(_) => {
                    if plan.quiet(&path, false)
                        && plan.unused(&path, true)
                        && super::leftovers::writers_ended(&path, plan)
                    {
                        plan.archive(
                            &path,
                            "early-names",
                            "earlier product directory retained unchanged, including user additions",
                        );
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => plan.leave(&path, error.to_string()),
            }
        }
        for directory in &self.runtime {
            let entries = super::entries(directory, plan);
            if entries.is_empty() {
                if directory.exists()
                    && files::entries(directory).is_ok_and(|entries| entries.is_empty())
                    && plan.quiet(directory, false)
                    && plan.unused(directory, true)
                {
                    // A copy interrupted after retiring its children still has
                    // this directory's identity and pending archive slot.
                    plan.archive(
                        directory,
                        "runtime",
                        "empty legacy coordination directory retained after its users ended",
                    );
                }
            } else {
                self.runtime_group(directory, &entries, plan);
            }
            if cfg!(windows)
                && let Some(parent) = directory.parent()
            {
                plan.empty_after(parent);
                if let Some(parent) = parent.parent() {
                    plan.empty_after(parent);
                }
            }
        }
        'pdf: for directory in &self.pdf {
            let entries = super::entries(directory, plan);
            let mut locks = Vec::new();
            for path in &entries {
                let name = path.file_name().and_then(|name| name.to_str()).unwrap_or("");
                if (name.starts_with(".pdfium") || name.starts_with(".libpdfium"))
                    && name.ends_with(".lock")
                    && std::fs::symlink_metadata(path)
                        .is_ok_and(|metadata| metadata.is_file() && !crate::disk::is_link(&metadata))
                {
                    match updates::lock_existing(path) {
                        Ok(Some(file)) => locks.push(file),
                        Ok(None) => {
                            plan.active(path, "Pdfium extraction lock is held; cache retirement deferred");
                            continue 'pdf;
                        }
                        Err(error) => {
                            plan.active(path, error.to_string());
                            continue 'pdf;
                        }
                    }
                }
            }
            if !locks.is_empty() && (!plan.quiet(directory, true) || !plan.unused(directory, true)) {
                continue;
            }
            plan.locks.extend(locks);
            for path in entries {
                let name = path.file_name().and_then(|name| name.to_str()).unwrap_or("");
                if let Some(expected) = pdf_hash(name) {
                    if !plan.quiet(&path, false) || !plan.unused(&path, true) {
                        continue;
                    }
                    let Some(snapshot) = plan.read(&path) else { continue };
                    if snapshot.bytes().is_some_and(|bytes| files::hash(bytes) == expected) && files::plain(&snapshot) {
                        plan.delete(snapshot, "Pdfium bytes exactly match the artifact pinned by released 0.x builds");
                    } else if plan.quiet(&path, false) && plan.unused(&path, true) {
                        plan.archive(&path, "pdf", "modified Pdfium cache retained unchanged");
                    }
                } else if (name.starts_with(".pdfium") || name.starts_with(".libpdfium"))
                    && plan.quiet(&path, false)
                    && plan.unused(&path, true)
                {
                    plan.archive(&path, "pdf", "Pdfium extraction state retained unchanged");
                }
            }
            plan.empty_after(directory);
        }
        for path in super::entries(&self.temp, plan) {
            let name = path.file_name().and_then(|name| name.to_str()).unwrap_or("");
            if name.starts_with("fastctx-command-") && name.ends_with(".sh") {
                if plan.quiet(&path, false) && plan.unused(&path, true) {
                    plan.archive(&path, "temporary", "obsolete command script retained unchanged");
                }
            } else if processes::nonce_pid(name, &["fastctx-npm-check-", "fastctx-npm-preflight-"]).is_some() {
                self.cache(&path, plan);
            }
        }
    }

    fn cache(&self, path: &Path, plan: &mut Plan) {
        let name = path.file_name().and_then(|name| name.to_str()).unwrap_or("");
        if let Some(pid) = processes::nonce_pid(name, &["fastctx-npm-check-", "fastctx-npm-preflight-"])
            && !plan.creator_ended(path, pid)
        {
            return;
        }
        if plan.quiet(path, false) && plan.unused(path, true) {
            plan.archive(path, "temporary", "unused npm query cache retained unchanged");
        }
    }

    fn runtime_group(&self, directory: &Path, entries: &[PathBuf], plan: &mut Plan) {
        #[cfg(target_os = "linux")]
        {
            let sockets = match std::fs::read_to_string("/proc/net/unix") {
                Ok(sockets) => sockets,
                Err(error) => {
                    plan.active(directory, format!("cannot inspect Unix socket use: {error}"));
                    return;
                }
            };
            for path in entries {
                if sockets
                    .lines()
                    .any(|line| line.split_whitespace().skip(7).collect::<Vec<_>>().join(" ") == path.to_string_lossy())
                {
                    plan.active(
                        path,
                        "Unix endpoint is still registered in the kernel; retained until its owner exits",
                    );
                    return;
                }
            }
        }
        if !plan.quiet(directory, false) || !plan.unused(directory, true) {
            return;
        }
        let directory_object = match archive::Object::read(directory) {
            Ok(Some(object)) => object,
            Ok(None) => return,
            Err(error) => {
                plan.active(directory, error.to_string());
                return;
            }
        };
        let mut locks = Vec::new();
        let mut names = Vec::new();
        for path in entries {
            if path.extension().is_some_and(|extension| extension == "lock") {
                let regular = std::fs::symlink_metadata(path)
                    .is_ok_and(|metadata| metadata.is_file() && !crate::disk::is_link(&metadata));
                if regular {
                    match updates::lock_in_directory(path) {
                        Ok(Some(lock)) => {
                            locks.push(lock);
                            names.push(path);
                        }
                        Ok(None) => {
                            plan.active(path, "legacy coordination lock is held; the group remains in place");
                            return;
                        }
                        Err(error) => {
                            plan.active(path, error.to_string());
                            return;
                        }
                    }
                }
            }
        }
        if !plan.quiet(directory, true) || !plan.unused(directory, true) {
            return;
        }
        // Verify names against the held handles without walking the same
        // parent chain for every file. The directory itself is checked once.
        for (path, lock) in names.into_iter().zip(&locks) {
            let same = (|| -> std::io::Result<bool> {
                let current = crate::disk::open(path)?;
                Ok(crate::disk::identity(&current)? == crate::disk::identity(lock)?)
            })();
            match same {
                Ok(true) => {}
                Ok(false) => {
                    plan.active(path, "runtime lock changed during inspection");
                    return;
                }
                Err(error) => {
                    plan.active(path, error.to_string());
                    return;
                }
            }
        }
        if let Err(error) = directory_object.verify() {
            plan.active(directory, error.to_string());
            return;
        }
        // All released processes were absent in both observations, with the
        // current locks held across the second one. This also covers home keys
        // left by earlier spellings of HOME, which cannot be reversed from hashes.
        // This entire directory belongs to the retired runtime. Moving it as
        // one object preserves added contents and avoids one journal and durable
        // rename per lock file. Publication decides when handles may be released.
        plan.runtime_locks.insert(directory_object.path.clone(), locks);
        plan.archive_object(
            directory_object,
            "runtime",
            "legacy coordination or discovery directory retained after its users ended".into(),
        );
    }
}

#[cfg(windows)]
fn runtime_dir(component: &str, temp: &Path) -> PathBuf {
    absolute_env("LOCALAPPDATA")
        .or_else(|| absolute_env("USERPROFILE").map(|home| home.join("AppData/Local")))
        .unwrap_or_else(|| temp.into())
        .join("fastctx/runtime")
        .join(component)
}

#[cfg(unix)]
fn runtime_dir(component: &str, temp: &Path) -> PathBuf {
    match absolute_env("XDG_RUNTIME_DIR") {
        Some(root) => root.join(format!("fastctx-{component}")),
        // SAFETY: geteuid has no preconditions.
        None => {
            let base = if temp.is_absolute() { temp } else { Path::new("/tmp") };
            base.join(format!("fastctx-{component}-{}", unsafe { libc::geteuid() }))
        }
    }
}

fn absolute_env(name: &str) -> Option<PathBuf> {
    env::var_os(name).map(PathBuf::from).filter(|path| path.is_absolute())
}

fn pdf_dir(home: &Path) -> Option<PathBuf> {
    if cfg!(windows) {
        absolute_env("LOCALAPPDATA").map(|path| path.join("fastctx"))
    } else if cfg!(target_os = "macos") {
        Some(home.join("Library/Caches/fastctx"))
    } else {
        Some(absolute_env("XDG_CACHE_HOME").unwrap_or_else(|| home.join(".cache")).join("fastctx"))
    }
}

// Artifact digests, not program code: the nine release tags' build.rs pins
// Chromium 7763. Windows ARM64 was added in 0.2.6.
fn pdf_hash(name: &str) -> Option<&'static str> {
    [
        (
            "pdfium-chromium-7763-a63949dc46a7314b.dll",
            "a63949dc46a7314bba619ac6cc1b3849627e137f542ae31b2b36b302841f77ae",
        ),
        (
            "pdfium-chromium-7763-682ee648af5629c1.dll",
            "682ee648af5629c1194bb3649e67252d162bdab06e00cded3e2ebbe88be7bf49",
        ),
        (
            "libpdfium-chromium-7763-9167f6d9190f217fa.so",
            "9167f6d9190f217fab5bfb864620108e280c124b7f7762cc4ef66e1078e0ec62",
        ),
        (
            "libpdfium-chromium-7763-b67d8bc289bf9916f.dylib",
            "b67d8bc289bf9916f697add53b163730ff22243ea896e97f942e09cb634e8a14",
        ),
        (
            "libpdfium-chromium-7763-0501a43035c44ccd4.dylib",
            "0501a43035c44ccd498d77c1bf7fb8aa88facdd0963d423f51cfea2d4d46f52b",
        ),
    ]
    .into_iter()
    .find_map(|(filename, hash)| (filename == name).then_some(hash))
}
