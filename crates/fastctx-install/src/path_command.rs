//! Native and Bash PATH lookups stay distinct on Windows.

use std::ffi::OsStr;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use fastctx_types::renewal::{PathCommand, PathLookup};

pub fn inspect(root: &Path) -> Vec<PathCommand> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let searches = if cfg!(windows) {
        let extensions = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
        vec![
            (
                PathLookup::Windows,
                extensions
                    .split(';')
                    .filter(|extension| !extension.is_empty())
                    .map(|extension| format!("fastctx{extension}"))
                    .collect::<Vec<_>>(),
            ),
            (PathLookup::Bash, vec!["fastctx".to_owned(), "fastctx.exe".to_owned()]),
        ]
    } else {
        vec![(PathLookup::Unix, vec!["fastctx".to_owned()])]
    };
    let mut commands: Vec<_> = searches
        .into_iter()
        .filter_map(|(lookup, names)| {
            let entry = std::env::split_paths(&path).find_map(|directory| {
                names.iter().map(|name| directory.join(name)).find(|candidate| executable(candidate))
            })?;
            Some(describe(root, lookup, &entry))
        })
        .collect();
    let codex = fastctx_platform::user_home().map_err(|error| error.to_string()).and_then(|home| {
        let environment = std::env::vars_os().collect::<Vec<_>>();
        let home = fastctx_hosts::codex::launch::codex_home(&home, &environment);
        crate::codex::portable_command(&home)
    });
    commands.push(match codex {
        Ok(Some(entry)) => describe(root, PathLookup::Codex, &entry),
        result => PathCommand {
            lookup: PathLookup::Codex,
            path: "fastctx".into(),
            executable: None,
            build: None,
            is_world: None,
            legacy_unapply: false,
            error: Some(match result {
                Err(error) => error,
                _ => "Codex cannot find command = fastctx in this process's environment".into(),
            }),
        },
    });
    commands
}

fn describe(root: &Path, lookup: PathLookup, entry: &Path) -> PathCommand {
    let resolved = resolve(entry);
    let (program, build, error) = match resolved {
        Ok(program) => match crate::program::inspect(&program) {
            Ok(build) => (Some(program), Some(build), None),
            Err(error) => (Some(program), None, Some(format!("{error:#}"))),
        },
        Err(error) => (None, None, Some(format!("{error:#}"))),
    };
    let stable = crate::executable::installed_path(root);
    let is_world =
        program.as_ref().map(|program| match (std::fs::canonicalize(program), std::fs::canonicalize(&stable)) {
            (Ok(program), Ok(stable)) => program == stable,
            _ => false,
        });
    let legacy_unapply = build.as_ref().is_some_and(|build| {
        matches!(
            build.version.as_str(),
            "0.1.0" | "0.1.1" | "0.2.0" | "0.2.1" | "0.2.2" | "0.2.3" | "0.2.4" | "0.2.5" | "0.2.6"
        )
    });
    PathCommand {
        lookup,
        path: entry.display().to_string(),
        executable: program.map(|program| program.display().to_string()),
        build,
        is_world,
        legacy_unapply,
        error,
    }
}

fn executable(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else { return false };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    return fastctx_platform::unix_executable::is_executable(path);
    #[cfg(not(unix))]
    true
}

fn resolve(entry: &Path) -> anyhow::Result<PathBuf> {
    let canonical = std::fs::canonicalize(entry)?;
    let native = if cfg!(windows) {
        canonical
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("exe") || extension.eq_ignore_ascii_case("com"))
    } else {
        let mut header = [0; 2];
        std::fs::File::open(&canonical)?.read_exact(&mut header)?;
        header != *b"#!"
    };
    if native {
        return Ok(canonical);
    }
    let text = std::fs::read_to_string(&canonical)?;
    if text.contains("${0%/*}/fastctx.exe") && entry.parent().is_some_and(|parent| parent.join("fastctx.exe").is_file())
    {
        return Ok(entry.parent().expect("entry parent").join("fastctx.exe"));
    }
    let prefix = entry.parent().ok_or_else(|| anyhow::anyhow!("PATH wrapper has no directory"))?;
    for name in ["fastctx", "codex-fastctx"] {
        let slash = format!("node_modules/{name}/launcher.js");
        let backslash = slash.replace('/', "\\");
        let direct = canonical.file_name() == Some(OsStr::new("launcher.js"))
            && canonical.parent().is_some_and(|parent| parent.file_name() == Some(OsStr::new(name)));
        if !direct && !text.contains(&slash) && !text.contains(&backslash) {
            continue;
        }
        let package = if direct {
            canonical.parent().expect("launcher parent").to_path_buf()
        } else {
            prefix.join("node_modules").join(name)
        };
        let metadata: serde_json::Value = serde_json::from_slice(&std::fs::read(package.join("package.json"))?)?;
        anyhow::ensure!(metadata["bin"]["fastctx"] == "launcher.js", "npm command does not name the fastctx launcher");
        let os = if cfg!(windows) {
            "win32"
        } else if cfg!(target_os = "macos") {
            "darwin"
        } else {
            "linux"
        };
        let arch = match std::env::consts::ARCH {
            "x86_64" => "x64",
            "aarch64" => "arm64",
            arch => arch,
        };
        let platform = format!("{os}-{arch}");
        for parent in package.ancestors() {
            let modules = if parent.file_name() == Some(OsStr::new("node_modules")) {
                parent.to_path_buf()
            } else {
                parent.join("node_modules")
            };
            let binary = modules.join("@fastctx").join(&platform).join("bin").join(crate::renewal::EXECUTABLE);
            if binary.is_file() {
                return Ok(binary);
            }
        }
        anyhow::bail!("npm launcher {} has no platform executable; its actual kernel is unknown", metadata["version"]);
    }
    anyhow::bail!("the PATH entry is an unrecognized wrapper; its actual kernel is unknown")
}
