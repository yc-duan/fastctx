//! Each agent's programs on this machine: where each installer and manager
//! puts them, with the version its manifest or its file records.

use std::path::{Path, PathBuf};

use fastctx_platform::file_version::file_version;
use fastctx_types::logins::{FoundProgram, Installed, Within};

use super::packages::{self, Modules};
use super::{Programs, Where, entries, same_file};

/// A program found, and the paths a terminal may find it by.
struct Found {
    program: FoundProgram,
    entries: Vec<PathBuf>,
}

/// Every program of both agents this user has here.
pub fn find(at: &Where) -> Programs {
    let mut unreadable = Vec::new();
    let modules = packages::modules(at, &mut unreadable);
    let codex = codex(at, &modules, &mut unreadable);
    let claude = claude(at, &modules, &mut unreadable);
    Programs {
        codex: on_path(at, codex, "codex", &mut unreadable),
        claude: on_path(at, claude, "claude", &mut unreadable),
        unreadable,
    }
}

fn exe(name: &str) -> String {
    if cfg!(windows) { format!("{name}.exe") } else { name.to_string() }
}

/// The version `path` records in its file, a failure to read it told.
fn recorded(path: &Path, unreadable: &mut Vec<String>) -> Option<String> {
    match file_version(path) {
        Ok(version) => version,
        Err(error) => {
            unreadable.push(format!("{}: {error}", path.display()));
            None
        }
    }
}

fn program(path: Option<PathBuf>, how: Installed, version: Option<String>, within: Option<Within>) -> FoundProgram {
    FoundProgram { path, how, version, within, on_path: false }
}

fn codex(at: &Where, modules: &[Modules], unreadable: &mut Vec<String>) -> Vec<Found> {
    let mut found = Vec::new();
    for package in packages::find(modules, "@openai/codex", "codex", unreadable) {
        let mut entries = vec![package.dir.clone()];
        entries.extend(package.command.clone());
        entries.extend(package.entry.clone());
        found.push(Found {
            program: program(package.entry.or(Some(package.dir)), package.how, package.version, None),
            entries,
        });
    }
    // Codex's own installer: an entry directory linked to the current
    // release kept in the default login's packages.
    let current = at.codex_home.join("packages").join("standalone").join("current");
    if current.exists() {
        let release = std::fs::canonicalize(&current).map(|release| super::plain(&release)).unwrap_or(current.clone());
        let version = std::fs::read(release.join("codex-package.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            .and_then(|manifest| manifest["version"].as_str().map(str::to_string));
        let binary = [release.join("bin").join(exe("codex")), release.join(exe("codex"))]
            .into_iter()
            .find(|path| path.is_file());
        let entry = at
            .var("CODEX_INSTALL_DIR")
            .unwrap_or_else(|| {
                if cfg!(windows) {
                    at.var("LOCALAPPDATA").unwrap_or_default().join("Programs").join("OpenAI").join("Codex").join("bin")
                } else {
                    at.user_home.join(".local").join("bin")
                }
            })
            .join(exe("codex"));
        let mut entries = vec![entry];
        entries.extend(binary.clone());
        found.push(Found { program: program(binary, Installed::Official, version, None), entries });
    }
    managed(at, &mut found, "codex", "OpenAI.Codex_", "codex", unreadable);
    if cfg!(windows) {
        for (app, version, dir) in windows_apps(at, "OpenAI.Codex_", unreadable) {
            let binary = dir.join("app").join("resources").join("codex.exe");
            let within = Within { name: app, id: "OpenAI.Codex".into(), version: Some(version), path: dir };
            found.push(Found {
                program: program(binary.is_file().then_some(binary.clone()), Installed::DesktopApp, None, Some(within)),
                entries: vec![binary],
            });
        }
    }
    for (editor, extension, version) in extensions(at, "openai.chatgpt-", "openai", "chatgpt", unreadable) {
        let platform = if cfg!(windows) {
            "windows"
        } else if cfg!(target_os = "macos") {
            "macos"
        } else {
            "linux"
        };
        let arch = if cfg!(target_arch = "aarch64") { "aarch64" } else { "x86_64" };
        let binary = extension.join("bin").join(format!("{platform}-{arch}")).join(exe("codex"));
        let within = Within { name: editor, id: "openai.chatgpt".into(), version, path: extension };
        found.push(Found {
            program: program(binary.is_file().then_some(binary.clone()), Installed::Extension, None, Some(within)),
            entries: vec![binary],
        });
    }
    found
}

fn claude(at: &Where, modules: &[Modules], unreadable: &mut Vec<String>) -> Vec<Found> {
    let mut found = Vec::new();
    for package in packages::find(modules, "@anthropic-ai/claude-code", "claude", unreadable) {
        let mut entries = vec![package.dir.clone()];
        entries.extend(package.command.clone());
        entries.extend(package.entry.clone());
        found.push(Found {
            program: program(package.entry.or(Some(package.dir)), package.how, package.version, None),
            entries,
        });
    }
    // Claude Code's own installer: a launcher in `~/.local/bin`, the same
    // file as one in its versions; on Windows the launcher records its
    // version, elsewhere it links to the version's file.
    let launcher = at.user_home.join(".local").join("bin").join(exe("claude"));
    if launcher.is_file() {
        let version = if cfg!(windows) {
            recorded(&launcher, unreadable)
        } else {
            std::fs::canonicalize(&launcher)
                .ok()
                .filter(|target| target.parent().and_then(Path::file_name).is_some_and(|name| name == "versions"))
                .and_then(|target| target.file_name().map(|name| name.to_string_lossy().into_owned()))
        };
        found.push(Found {
            program: program(Some(launcher.clone()), Installed::Official, version, None),
            entries: vec![launcher],
        });
    }
    // Its first installer's copy, inside the default login.
    let local = at.claude_home.join("local");
    if let Some((version, entry)) = packages::manifest(
        &local.join("node_modules").join("@anthropic-ai").join("claude-code"),
        "@anthropic-ai/claude-code",
        unreadable,
    ) {
        let wrapper = local.join("claude");
        let path = if wrapper.is_file() { Some(wrapper.clone()) } else { entry.clone() };
        found.push(Found { program: program(path, Installed::Official, version, None), entries: vec![wrapper] });
    }
    managed(at, &mut found, "claude-code", "Anthropic.ClaudeCode_", "claude", unreadable);
    if cfg!(windows) {
        for (app, version, dir) in windows_apps(at, "Claude_", unreadable) {
            let within = Within { name: app, id: "Claude".into(), version: Some(version), path: dir };
            // The app downloads its Claude Code into its own data, kept
            // where an MSIX app's roaming data is, or the ordinary one.
            let mut kept = Vec::new();
            let mut data = Vec::new();
            if let Some(local) = at.var("LOCALAPPDATA") {
                data.push(
                    local
                        .join("Packages")
                        .join("Claude_pzs8sxrjxfjjc")
                        .join("LocalCache")
                        .join("Roaming")
                        .join("Claude"),
                );
            }
            if let Some(roaming) = at.var("APPDATA") {
                data.push(roaming.join("Claude"));
            }
            for data in data {
                for version in entries(&data.join("claude-code"), unreadable) {
                    let binary = version.path().join("claude.exe");
                    if binary.is_file() {
                        let recorded = recorded(&binary, unreadable)
                            .or_else(|| Some(version.file_name().to_string_lossy().into_owned()));
                        kept.push(Found {
                            program: program(
                                Some(binary.clone()),
                                Installed::DesktopApp,
                                recorded,
                                Some(within.clone()),
                            ),
                            entries: vec![binary],
                        });
                    }
                }
            }
            if kept.is_empty() {
                kept.push(Found {
                    program: program(None, Installed::DesktopApp, None, Some(within)),
                    entries: Vec::new(),
                });
            }
            found.extend(kept);
        }
    }
    for (editor, extension, version) in extensions(at, "anthropic.claude-code-", "anthropic", "claude-code", unreadable)
    {
        let resources = extension.join("resources");
        let mut binaries = vec![resources.join("native-binary").join(exe("claude"))];
        binaries.extend(
            entries(&resources.join("native-binaries"), unreadable)
                .into_iter()
                .map(|platform| platform.path().join(exe("claude"))),
        );
        let binary = binaries.into_iter().find(|path| path.is_file());
        let script = resources.join("claude-code").join("cli.js");
        let cli_version = match &binary {
            Some(binary) => recorded(binary, unreadable),
            None => packages::manifest(&resources.join("claude-code"), "@anthropic-ai/claude-code", unreadable)
                .and_then(|(version, _)| version),
        };
        let path = binary.or_else(|| script.is_file().then_some(script));
        let within = Within { name: editor, id: "anthropic.claude-code".into(), version, path: extension };
        found.push(Found {
            entries: path.iter().cloned().collect(),
            program: program(path, Installed::Extension, cli_version, Some(within)),
        });
    }
    found
}

/// The agent's program from WinGet, Scoop, Chocolatey or Homebrew, whose
/// package is `scoop` in Scoop, Chocolatey and Homebrew, `winget` the start
/// of WinGet's package directory, and whose program is named `command`.
fn managed(at: &Where, found: &mut Vec<Found>, scoop: &str, winget: &str, command: &str, unreadable: &mut Vec<String>) {
    let binary = exe(command);
    if cfg!(windows) {
        let mut roots = Vec::new();
        if let Some(local) = at.var("LOCALAPPDATA") {
            roots.push(local.join("Microsoft").join("WinGet").join("Packages"));
        }
        if let Some(programs) = at.var("ProgramFiles") {
            roots.push(programs.join("WinGet").join("Packages"));
        }
        for root in roots {
            for package in entries(&root, unreadable) {
                if !package.file_name().to_string_lossy().starts_with(winget) {
                    continue;
                }
                // WinGet keeps a portable program as the release names it.
                let program_file =
                    entries(&package.path(), unreadable).into_iter().map(|entry| entry.path()).find(|path| {
                        path.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
                            && path
                                .file_name()
                                .is_some_and(|name| name.to_string_lossy().to_lowercase().starts_with(command))
                    });
                let version = program_file.as_ref().and_then(|path| recorded(path, unreadable));
                found.push(Found {
                    entries: program_file.iter().cloned().collect(),
                    program: program(program_file, Installed::Winget, version, None),
                });
            }
        }
        let mut scoops: Vec<PathBuf> = at.var("SCOOP").into_iter().collect();
        scoops.push(at.user_home.join("scoop"));
        scoops.extend(at.var("SCOOP_GLOBAL"));
        scoops.extend(at.var("ProgramData").map(|data| data.join("scoop")));
        let mut seen: Vec<PathBuf> = Vec::new();
        for root in scoops {
            let current = root.join("apps").join(scoop).join("current");
            if !current.is_dir() || seen.iter().any(|known| same_file(known, &current)) {
                continue;
            }
            seen.push(current.clone());
            let version = std::fs::read(current.join("manifest.json"))
                .ok()
                .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
                .and_then(|manifest| manifest["version"].as_str().map(str::to_string));
            let path =
                [current.join(&binary), current.join("bin").join(&binary)].into_iter().find(|path| path.is_file());
            found.push(Found {
                entries: path.iter().cloned().chain([root.join("shims").join(&binary)]).collect(),
                program: program(path, Installed::Scoop, version, None),
            });
        }
        let chocolatey =
            at.var("ChocolateyInstall").or_else(|| at.var("ProgramData").map(|data| data.join("chocolatey")));
        if let Some(chocolatey) = chocolatey {
            let lib = chocolatey.join("lib").join(scoop);
            if lib.is_dir() {
                let version = std::fs::read_to_string(lib.join(format!("{scoop}.nuspec"))).ok().and_then(|text| {
                    let start = text.find("<version>")? + "<version>".len();
                    let end = text[start..].find("</version>")? + start;
                    Some(text[start..end].trim().to_string())
                });
                let path = Some(lib.join("tools").join(&binary)).filter(|path| path.is_file());
                found.push(Found {
                    entries: path.iter().cloned().chain([chocolatey.join("bin").join(&binary)]).collect(),
                    program: program(path, Installed::Chocolatey, version, None),
                });
            }
        }
    } else {
        let prefixes = [
            PathBuf::from("/opt/homebrew"),
            PathBuf::from("/usr/local"),
            PathBuf::from("/home/linuxbrew/.linuxbrew"),
            at.user_home.join(".linuxbrew"),
        ];
        for prefix in prefixes {
            for cask in [scoop.to_string(), format!("{scoop}@latest")] {
                for version in entries(&prefix.join("Caskroom").join(&cask), unreadable) {
                    let dir = version.path();
                    let path =
                        [dir.join(&binary), dir.join("bin").join(&binary)].into_iter().find(|path| path.is_file());
                    found.push(Found {
                        entries: path.iter().cloned().chain([prefix.join("bin").join(&binary)]).collect(),
                        program: program(
                            path,
                            Installed::Homebrew,
                            Some(version.file_name().to_string_lossy().into_owned()),
                            None,
                        ),
                    });
                }
            }
        }
    }
}

/// The desktop apps installed from the Microsoft Store whose package names
/// begin `prefix`: each one's display name, version and directory.
fn windows_apps(at: &Where, prefix: &str, unreadable: &mut Vec<String>) -> Vec<(String, String, PathBuf)> {
    let Some(programs) = at.var("ProgramFiles") else { return Vec::new() };
    let mut apps = Vec::new();
    for package in entries(&programs.join("WindowsApps"), unreadable) {
        let name = package.file_name().to_string_lossy().into_owned();
        let Some(rest) = name.strip_prefix(prefix) else { continue };
        // `<name>_<version>_<architecture>__<publisher>`; resource packs
        // are `_neutral_split…` and hold no program.
        let mut parts = rest.split('_');
        let (Some(version), Some(architecture)) = (parts.next(), parts.next()) else { continue };
        if architecture == "neutral" || version.is_empty() {
            continue;
        }
        let dir = package.path();
        let display = std::fs::read_to_string(dir.join("AppxManifest.xml")).ok().and_then(|text| {
            let start = text.find("<DisplayName>")? + "<DisplayName>".len();
            let end = text[start..].find("</DisplayName>")? + start;
            Some(text[start..end].trim().to_string())
        });
        apps.push((display.unwrap_or_else(|| prefix.trim_end_matches('_').to_string()), version.to_string(), dir));
    }
    apps
}

/// The editors whose extensions are kept in a directory of the user's
/// home: the editor's name and that directory.
const EDITORS: &[(&str, &str)] = &[
    ("VS Code", ".vscode"),
    ("VS Code Insiders", ".vscode-insiders"),
    ("VS Code Server", ".vscode-server"),
    ("VS Code Server Insiders", ".vscode-server-insiders"),
    ("Cursor", ".cursor"),
    ("Cursor Server", ".cursor-server"),
    ("Windsurf", ".windsurf"),
    ("Windsurf Server", ".windsurf-server"),
    ("VSCodium", ".vscode-oss"),
    ("VSCodium Insiders", ".vscodium-insiders"),
    ("Antigravity", ".antigravity"),
    ("Antigravity", ".antigravity-ide"),
];

/// Each installed extension of `publisher`'s `name` whose directory begins
/// `prefix`: the editor, the extension's directory and its version. An
/// editor's list of extensions it no longer uses (`.obsolete`) is heeded.
fn extensions(
    at: &Where,
    prefix: &str,
    publisher: &str,
    name: &str,
    unreadable: &mut Vec<String>,
) -> Vec<(String, PathBuf, Option<String>)> {
    let mut found = Vec::new();
    for (editor, dir) in EDITORS {
        let root = at.user_home.join(dir).join("extensions");
        let obsolete: serde_json::Value = std::fs::read(root.join(".obsolete"))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        for extension in entries(&root, unreadable) {
            let directory = extension.file_name().to_string_lossy().into_owned();
            if !directory.to_lowercase().starts_with(prefix) || obsolete.get(&directory).is_some() {
                continue;
            }
            let path = extension.path();
            let manifest = std::fs::read(path.join("package.json"))
                .ok()
                .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok());
            let Some(manifest) = manifest else { continue };
            let ours = manifest["publisher"].as_str().is_some_and(|value| value.eq_ignore_ascii_case(publisher))
                && manifest["name"].as_str().is_some_and(|value| value.eq_ignore_ascii_case(name));
            if ours {
                found.push(((*editor).to_string(), path, manifest["version"].as_str().map(str::to_string)));
            }
        }
    }
    found
}

/// Marks the program a terminal starts for `command`: the first on the
/// user's search path. One found there that no installer above accounts
/// for is listed as found on the path.
fn on_path(at: &Where, mut found: Vec<Found>, command: &str, unreadable: &mut Vec<String>) -> Vec<FoundProgram> {
    let names: Vec<String> = if cfg!(windows) {
        vec![format!("{command}.exe"), format!("{command}.cmd"), format!("{command}.ps1"), command.to_string()]
    } else {
        vec![command.to_string()]
    };
    let first =
        at.path().into_iter().find_map(|dir| names.iter().map(|name| dir.join(name)).find(|path| path.is_file()));
    if let Some(first) = first {
        let known = found.iter_mut().find(|program| program.entries.iter().any(|entry| same_file(entry, &first)));
        match known {
            Some(program) => program.program.on_path = true,
            None => {
                let version = recorded(&first, unreadable);
                let mut program = program(Some(first.clone()), Installed::Path, version, None);
                program.on_path = true;
                found.push(Found { program, entries: vec![first] });
            }
        }
    }
    let mut programs: Vec<FoundProgram> = Vec::new();
    for one in found {
        let duplicate = programs.iter().any(|known| match (&known.path, &one.program.path) {
            (Some(a), Some(b)) => same_file(a, b),
            _ => false,
        });
        if !duplicate {
            programs.push(one.program);
        }
    }
    programs
}
