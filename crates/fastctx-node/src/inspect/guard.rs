//! What a read-only look never reads: a World's own directory, where a
//! machine's identity and its keys are; the places and files that hold
//! credentials or the commands typed with them; the kernel's own files; and
//! network shares and devices, which opening would reach out of this
//! machine's disks, handing its login to whoever answers. Everything a look
//! reads goes to a model, and none of these has reason to.

use std::path::{Component, Path, PathBuf};

use fastctx_types::ipc::Failure;

/// Folders whose contents are credentials, wherever they are.
const FOLDERS: &[&str] = &[
    ".ssh",
    ".gnupg",
    ".aws",
    ".azure",
    ".kube",
    ".docker",
    ".password-store",
    ".mozilla",
    "google-chrome",
    "chromium",
    "keychains",
    "keyrings",
];
/// How the folder a World keeps in a home begins: `.fastctx`, and the
/// `.fastctx-dev` of a development build beside it.
const WORLD: &str = ".fastctx";
/// Files that hold credentials, by name.
const FILES: &[&str] = &[
    ".netrc",
    "_netrc",
    ".git-credentials",
    ".npmrc",
    ".pypirc",
    ".pgpass",
    ".my.cnf",
    ".env",
    ".envrc",
    ".htpasswd",
    "rclone.conf",
    "shadow",
    "gshadow",
    "shadow-",
    "master.passwd",
    "id_rsa",
    "id_dsa",
    "id_ecdsa",
    "id_ed25519",
    "id_ecdsa_sk",
    "id_ed25519_sk",
    "cookies",
    "cookies.sqlite",
    "login data",
    "web data",
    "logins.json",
    "key4.db",
    "consolehost_history.txt",
    "fish_history",
];
/// Words that, in a file's name, mark it as holding credentials.
const NAMED: &[&str] = &["credential", "secret", "password", "creds", "oauth"];
/// Kinds of file that hold keys, by extension.
const EXTENSIONS: &[&str] = &["pem", "key", "p12", "pfx", "ppk", "kdbx", "jks", "keystore", "keytab", "gpg", "age"];
/// A folder and what in it holds credentials: the agents' own logins among
/// them.
const WITHIN: &[(&str, &str)] = &[
    (".codex", "auth.json"),
    (".config", "gcloud"),
    ("gh", "hosts.yml"),
    ("google", "chrome"),
    ("microsoft", "edge"),
    ("mozilla", "firefox"),
    ("bravesoftware", "brave-browser"),
    ("microsoft", "credentials"),
    ("microsoft", "protect"),
    (".m2", "settings.xml"),
    (".gradle", "gradle.properties"),
];
/// The kernel's own files on Unix, which a look leaves to the processes and
/// the machine's resources it shows instead.
#[cfg(unix)]
const KERNEL: &[&str] = &["/proc", "/sys", "/dev"];

/// Where a look may and may not read on this machine.
pub(super) struct Guard {
    /// The World's own directory, as this machine resolves it.
    world: Option<PathBuf>,
    /// The folder of a conversation's evidence a goal's judge may read in it.
    evidence: Option<PathBuf>,
}

/// A path a look may read, or that is not there.
pub(super) enum Opened {
    Found(PathBuf),
    Missing,
}

impl Guard {
    pub(super) fn new(world: &Path, evidence: Option<PathBuf>) -> Self {
        Self {
            world: fastctx_platform::disk::canonicalize(world).ok(),
            evidence: evidence.and_then(|evidence| fastctx_platform::disk::canonicalize(evidence).ok()),
        }
    }

    /// `path` resolved, its links followed, once neither it nor where it
    /// leads is one a look may not read. What the path names is judged
    /// before the disk is touched, so a share is never reached; where it
    /// leads, once it is resolved and can be compared with the World's own
    /// directories, which are resolved too.
    pub(super) fn open(&self, path: &str) -> Result<Opened, Failure> {
        let given =
            fastctx_platform::local_path::local_path(path).map_err(|error| Failure::not_run(error.to_string()))?;
        if !given.is_absolute() {
            return Err(Failure::not_run(format!(
                "{path} is not an absolute path as this machine writes them (/… on Unix, C:/… on Windows)"
            )));
        }
        if !on_this_machine(&given) {
            return Err(Failure::not_run(format!(
                "{path} names a network share or a device, not a file on this machine's disks; a look opens only those"
            )));
        }
        if leads_off(&given, 0) {
            return Err(Failure::not_run(format!(
                "{path} leads through a link to a network share or a device; a look opens only files on this machine's disks"
            )));
        }
        refuse(path, &given)?;
        let resolved = match fastctx_platform::disk::canonicalize(&given) {
            Ok(resolved) => resolved,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Opened::Missing),
            Err(error) => return Err(Failure::not_run(format!("{path}: {error}"))),
        };
        if a_device(&resolved) {
            return Err(Failure::not_run(format!("{path} leads to a device, not a file; a look opens only files")));
        }
        refuse(path, &resolved)?;
        let a_world = names(&resolved).iter().any(|name| name.starts_with(WORLD))
            || self.world.as_ref().is_some_and(|world| resolved.starts_with(world))
            || in_a_world(&resolved);
        let evidence = self.evidence.as_ref().is_some_and(|evidence| resolved.starts_with(evidence));
        if a_world && !evidence {
            return Err(Failure::not_run(format!(
                "{path} is in a World's own directory, which a look does not read; a job's output is read by its id"
            )));
        }
        Ok(Opened::Found(resolved))
    }
}

/// Refuses `path`, written `at` or resolved to it, where it is one of the
/// kernel's own files or holds credentials.
fn refuse(path: &str, at: &Path) -> Result<(), Failure> {
    if kernel(at) {
        return Err(Failure::not_run(format!(
            "{path} is one of the kernel's own files, which a look does not read; see the processes or the machine's resources instead"
        )));
    }
    if credentials(&names(at)) {
        return Err(Failure::not_run(format!("{path} holds credentials, which a look does not read")));
    }
    Ok(())
}

/// The names `at` passes through, in lower case.
fn names(at: &Path) -> Vec<String> {
    at.components()
        .filter_map(|component| match component {
            Component::Normal(name) => Some(name.to_string_lossy().to_lowercase()),
            _ => None,
        })
        .collect()
}

/// Whether `names`, a path's, lead into a place or a file of credentials, or
/// a shell's history of commands, which keeps the keys typed into them.
fn credentials(names: &[String]) -> bool {
    let file = names.last().map(String::as_str).unwrap_or_default();
    let (stem, extension) = match file.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => (stem, Some(extension)),
        _ => (file, None),
    };
    names.iter().any(|name| FOLDERS.contains(&name.as_str()))
        || names.windows(2).any(|pair| WITHIN.contains(&(pair[0].as_str(), pair[1].as_str())))
        || FILES.contains(&file)
        || file.starts_with(".env.")
        || NAMED.iter().any(|word| file.contains(word))
        || stem.ends_with("token")
        || stem.ends_with("tokens")
        || file.ends_with("_history")
        || file.ends_with(".history")
        || extension.is_some_and(|extension| EXTENSIONS.contains(&extension))
}

/// Whether a folder `at` is in, or `at` itself, is a World's own directory,
/// wherever its home put it: the folder `world` holding a machine's key.
fn in_a_world(at: &Path) -> bool {
    at.ancestors().any(|folder| {
        folder.file_name().is_some_and(|name| name.eq_ignore_ascii_case("world"))
            && fastctx_platform::disk::is_file(folder.join("device.key"))
    })
}

/// Whether the path written `at` is on one of this machine's disks: on
/// Windows, a drive's path, never a share (`\\host\share`, `//host/share`),
/// a device (`\\.\…`) or a file's alternate stream (`notes.txt:hidden`). A
/// drive the user mapped to a share is theirs, and is read as `run` would.
#[cfg(windows)]
fn on_this_machine(at: &Path) -> bool {
    use std::path::Prefix;
    let mut components = at.components();
    let drive = matches!(
        components.next(),
        Some(Component::Prefix(prefix)) if matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_))
    );
    drive && !components.any(|component| stream(&component))
}

#[cfg(not(windows))]
fn on_this_machine(_at: &Path) -> bool {
    true
}

/// Whether a link on the way to `at` leads off this machine's disks, which
/// following it, as resolving the path would, reaches: on Windows, links
/// are read one by one from the drive down, each before what lies past it is
/// touched. `depth` counts the links followed to get here.
#[cfg(windows)]
fn leads_off(at: &Path, depth: u8) -> bool {
    const LINKS: u8 = 32;
    if depth > LINKS {
        return true;
    }
    let mut ancestors: Vec<&Path> = at.ancestors().collect();
    ancestors.reverse();
    for folder in ancestors {
        let Ok(metadata) = fastctx_platform::disk::symlink_metadata(folder) else { return false };
        if !metadata.file_type().is_symlink() {
            continue;
        }
        let Ok(target) = fastctx_platform::disk::read_link(folder) else { return true };
        // Joined to the link's folder, a target relative to it, or to its
        // drive's root, becomes whole; a whole one stays as it is.
        let target = folder.parent().map_or_else(|| target.clone(), |parent| parent.join(&target));
        if !on_this_machine(&target) || leads_off(&target, depth + 1) {
            return true;
        }
    }
    false
}

#[cfg(not(windows))]
fn leads_off(_at: &Path, _depth: u8) -> bool {
    false
}

/// Whether a resolved path leads to a device (`C:/notes/CON` leads to the
/// console) or a file's alternate stream.
#[cfg(windows)]
fn a_device(at: &Path) -> bool {
    use std::path::Prefix;
    let mut components = at.components();
    matches!(components.next(), Some(Component::Prefix(prefix)) if matches!(prefix.kind(), Prefix::DeviceNS(_)))
        || components.any(|component| stream(&component))
}

#[cfg(not(windows))]
fn a_device(_at: &Path) -> bool {
    false
}

/// Whether `component` names a file's alternate stream, the one place a
/// colon stands in a Windows path past its drive.
#[cfg(windows)]
fn stream(component: &Component) -> bool {
    matches!(component, Component::Normal(name) if name.to_string_lossy().contains(':'))
}

#[cfg(unix)]
fn kernel(at: &Path) -> bool {
    KERNEL.iter().any(|folder| at.starts_with(folder))
}

#[cfg(not(unix))]
fn kernel(_at: &Path) -> bool {
    false
}
