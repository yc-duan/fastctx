//! The desktop app is a Windows package whose folder names its version, as
//! `OpenAI.Codex_26.1103.3517.0_x64__2p2nqsd0c76g0`. An upgrade installs the
//! new version beside the old one and removes the old, while the paths the
//! app wrote into the Codex configuration name the old version until the new
//! one first runs and writes them again. A path into a removed version is
//! followed to the same file in the newest installed version of the same
//! package, as Windows registers it; another installation of Codex is never
//! taken for it. A version only staged for a later upgrade is not
//! registered yet, and is used only where Windows cannot be asked.

use std::path::{Path, PathBuf};

/// A package's folder name: `<name>_<version>_<architecture>_<resource>_<publisher>`.
struct Package<'a> {
    name: &'a str,
    version: [u32; 4],
    architecture: &'a str,
    resource: &'a str,
    publisher: &'a str,
}

impl Package<'_> {
    fn parse(folder: &str) -> Option<Package<'_>> {
        let mut parts = folder.split('_');
        let (name, version, architecture, resource, publisher) =
            (parts.next()?, parts.next()?, parts.next()?, parts.next()?, parts.next()?);
        if parts.next().is_some() || name.is_empty() || publisher.is_empty() {
            return None;
        }
        let mut numbers = version.split('.').map(|number| number.parse::<u32>().ok());
        let version = [numbers.next()??, numbers.next()??, numbers.next()??, numbers.next()??];
        if numbers.next().is_some() {
            return None;
        }
        Some(Package { name, version, architecture, resource, publisher })
    }

    fn same_app(&self, other: &Package<'_>) -> bool {
        self.name == other.name
            && self.architecture == other.architecture
            && self.resource == other.resource
            && self.publisher == other.publisher
    }
}

fn package_of(folder: &Path) -> Option<Package<'_>> {
    folder.file_name()?.to_str().and_then(Package::parse)
}

/// When `path` lies in a version of a package that is no longer installed:
/// the folder of that version, and the folder of the newest installed
/// version of the same package holding the same file.
pub(super) fn moved(path: &Path) -> Option<(PathBuf, PathBuf)> {
    if path.exists() {
        return None;
    }
    let folder = path.ancestors().find(|ancestor| package_of(ancestor).is_some())?;
    let named = package_of(folder)?;
    let within = path.strip_prefix(folder).ok()?;
    let (_, current) = installed(folder, named.name)
        .into_iter()
        .filter_map(|place| {
            let version = package_of(&place).filter(|package| package.same_app(&named))?.version;
            place.join(within).exists().then_some((version, place))
        })
        .max_by_key(|(version, _)| *version)?;
    Some((folder.to_path_buf(), current))
}

/// The folders of the installed versions of the package `name`, as Windows
/// registers them; where it cannot be asked, the folders beside `folder`.
fn installed(folder: &Path, name: &str) -> Vec<PathBuf> {
    if let Some(registered) = registered(name) {
        return registered;
    }
    let Some(parent) = folder.parent() else { return Vec::new() };
    std::fs::read_dir(parent)
        .map(|entries| entries.filter_map(Result::ok).map(|entry| entry.path()).collect())
        .unwrap_or_default()
}

/// Asks Windows where the package `name` is installed. A user who is not an
/// administrator cannot list the folder of all packages, but can ask this.
#[cfg(windows)]
fn registered(name: &str) -> Option<Vec<PathBuf>> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    if !name.chars().all(|character| character.is_ascii_alphanumeric() || matches!(character, '.' | '-')) {
        return None;
    }
    let query = format!("Get-AppxPackage -Name '{name}' | ForEach-Object {{ $_.InstallLocation }}");
    let mut command = std::process::Command::new("powershell.exe");
    command.args(["-NoProfile", "-NonInteractive", "-Command", &query]).creation_flags(CREATE_NO_WINDOW);
    let output = if fastctx_platform::read_budget::active() {
        use std::process::{Output, Stdio};
        let mut child = command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
        let Some(mut pipe) = child.stdout.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        };
        let stdout = match fastctx_platform::read_budget::from_reader(&mut pipe, Path::new("PowerShell package list")) {
            Ok(stdout) => stdout,
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        };
        Output { status: child.wait().ok()?, stdout, stderr: Vec::new() }
    } else {
        command.output().ok()?
    };
    output.status.success().then(|| {
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(PathBuf::from)
            .collect()
    })
}

#[cfg(not(windows))]
fn registered(_name: &str) -> Option<Vec<PathBuf>> {
    None
}

/// Rewrites every path under `from` in `value` to the same place under `to`,
/// and gives each rewritten setting as a Codex `-c` override below `key`.
pub(super) fn rebase(value: &mut toml::Value, from: &Path, to: &Path, key: &str) -> Vec<String> {
    let (Some(from), Some(to)) = (from.to_str(), to.to_str()) else { return Vec::new() };
    let mut overrides = Vec::new();
    rebase_into(value, from, to, key, &mut overrides);
    overrides
}

fn rebase_into(value: &mut toml::Value, from: &str, to: &str, key: &str, overrides: &mut Vec<String>) {
    let changed = match value {
        toml::Value::Table(table) => {
            for (name, inner) in table.iter_mut() {
                rebase_into(inner, from, to, &format!("{key}.{name}"), overrides);
            }
            false
        }
        toml::Value::String(text) if text.contains(from) => {
            *text = text.replace(from, to);
            true
        }
        // An array is overridden whole.
        toml::Value::Array(items) if items.iter().any(|item| item.as_str().is_some_and(|text| text.contains(from))) => {
            for item in items.iter_mut() {
                if let toml::Value::String(text) = item {
                    *text = text.replace(from, to);
                }
            }
            true
        }
        _ => false,
    };
    if changed {
        overrides.push(format!("{key}={value}"));
    }
}
