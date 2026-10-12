//! The agents' Node packages, in the global modules of npm, pnpm, Yarn and
//! Bun, each manager's directory as its defaults and the environment
//! variables that move it say.

use std::path::{Path, PathBuf};

use fastctx_types::logins::Installed;

use super::Where;

/// One manager's global modules, and where it puts the commands that start
/// them.
pub(super) struct Modules {
    pub how: Installed,
    pub modules: PathBuf,
    pub bin: Option<PathBuf>,
}

/// A package of an agent found in a manager's modules.
pub(super) struct Package {
    pub how: Installed,
    pub dir: PathBuf,
    pub version: Option<String>,
    /// The program its manifest names to run.
    pub entry: Option<PathBuf>,
    /// The command the manager made for it, which a terminal finds.
    pub command: Option<PathBuf>,
}

/// Every manager's global modules this user may have.
pub(super) fn modules(at: &Where, unreadable: &mut Vec<String>) -> Vec<Modules> {
    let home = at.user_home;
    let mut found = Vec::new();
    let mut add = |how, modules: PathBuf, bin: Option<PathBuf>| {
        if !found.iter().any(|known: &Modules| super::same_file(&known.modules, &modules)) {
            found.push(Modules { how, modules, bin });
        }
    };
    let prefix = at.var("NPM_CONFIG_PREFIX");
    if cfg!(windows) {
        let npm = prefix.or_else(|| at.var("APPDATA").map(|data| data.join("npm")));
        if let Some(npm) = npm {
            add(Installed::Npm, npm.join("node_modules"), Some(npm));
        }
        let local = at.var("LOCALAPPDATA");
        let pnpm = at.var("PNPM_HOME").or_else(|| local.as_ref().map(|local| local.join("pnpm")));
        if let Some(pnpm) = pnpm {
            for store in super::entries(&pnpm.join("global"), unreadable) {
                add(Installed::Pnpm, store.path().join("node_modules"), Some(pnpm.clone()));
            }
        }
        if let Some(local) = &local {
            let yarn = local.join("Yarn");
            add(Installed::Yarn, yarn.join("Data").join("global").join("node_modules"), Some(yarn.join("bin")));
        }
    } else {
        if let Some(prefix) = prefix {
            add(Installed::Npm, prefix.join("lib").join("node_modules"), Some(prefix.join("bin")));
        }
        for prefix in [PathBuf::from("/usr/local"), PathBuf::from("/usr"), home.join(".npm-global")] {
            add(Installed::Npm, prefix.join("lib").join("node_modules"), Some(prefix.join("bin")));
        }
        for version in super::entries(&home.join(".nvm").join("versions").join("node"), unreadable) {
            let prefix = version.path();
            add(Installed::Npm, prefix.join("lib").join("node_modules"), Some(prefix.join("bin")));
        }
        let data = at.var("XDG_DATA_HOME").unwrap_or_else(|| home.join(".local").join("share"));
        let pnpm = at.var("PNPM_HOME").unwrap_or_else(|| {
            if cfg!(target_os = "macos") { home.join("Library").join("pnpm") } else { data.join("pnpm") }
        });
        for store in super::entries(&pnpm.join("global"), unreadable) {
            add(Installed::Pnpm, store.path().join("node_modules"), Some(pnpm.clone()));
        }
        let yarn = at.var("XDG_CONFIG_HOME").unwrap_or_else(|| home.join(".config")).join("yarn");
        add(Installed::Yarn, yarn.join("global").join("node_modules"), Some(home.join(".yarn").join("bin")));
    }
    let bun = at.var("BUN_INSTALL").unwrap_or_else(|| home.join(".bun"));
    add(Installed::Bun, bun.join("install").join("global").join("node_modules"), Some(bun.join("bin")));
    // A directory on the search path holding the agent's command and
    // global modules beside it is an npm prefix too (nvm, a custom one).
    for dir in at.path() {
        let modules =
            if cfg!(windows) { dir.join("node_modules") } else { dir.with_file_name("lib").join("node_modules") };
        if modules.is_dir() {
            add(Installed::Npm, modules, Some(dir));
        }
    }
    found
}

/// The package `name` (`@openai/codex`) in each of `modules`, with the
/// command the manager made for it, `command` (`codex`).
pub(super) fn find(modules: &[Modules], name: &str, command: &str, unreadable: &mut Vec<String>) -> Vec<Package> {
    let mut packages = Vec::new();
    for found in modules {
        let dir = name.split('/').fold(found.modules.clone(), |dir, part| dir.join(part));
        let Some((version, entry)) = manifest(&dir, name, unreadable) else { continue };
        let command = found.bin.as_ref().and_then(|bin| {
            let names: &[String] = &if cfg!(windows) {
                vec![format!("{command}.cmd"), format!("{command}.ps1"), command.to_string()]
            } else {
                vec![command.to_string()]
            };
            names.iter().map(|name| bin.join(name)).find(|path| path.is_file())
        });
        if packages.iter().any(|known: &Package| super::same_file(&known.dir, &dir)) {
            continue;
        }
        packages.push(Package { how: found.how, dir, version, entry, command });
    }
    packages
}

/// The version and program of the package in `dir`, when it is `name`.
pub(super) fn manifest(
    dir: &Path,
    name: &str,
    unreadable: &mut Vec<String>,
) -> Option<(Option<String>, Option<PathBuf>)> {
    let path = dir.join("package.json");
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => {
            unreadable.push(format!("{}: {error}", path.display()));
            return None;
        }
    };
    let value: serde_json::Value = match serde_json::from_slice(&bytes) {
        Ok(value) => value,
        Err(error) => {
            unreadable.push(format!("{}: {error}", path.display()));
            return None;
        }
    };
    if value["name"].as_str() != Some(name) {
        return None;
    }
    let version = value["version"].as_str().map(str::to_string);
    let entry = match &value["bin"] {
        serde_json::Value::String(entry) => Some(entry.as_str()),
        serde_json::Value::Object(bins) => bins.values().find_map(serde_json::Value::as_str),
        _ => None,
    }
    .map(|entry| dir.join(entry));
    Some((version, entry))
}
