use std::path::{Path, PathBuf};

use crate::apply::Places;

use super::{Plan, files, processes};

pub(super) fn inspect(places: &Places, old_root: &Path, plan: &mut Plan) {
    let mut targets = vec![places.fastctx.clone()];
    // Requests supply candidate coordinates, never deletion authority. Only
    // the current program or a separately verified published image can anchor
    // sibling cleanup; Node/npm/launcher directories are not installation roots.
    for path in super::entries(&old_root.join("update"), plan) {
        let name = path.file_name().and_then(|name| name.to_str()).unwrap_or("");
        if !name.starts_with("request-") || !name.ends_with(".json") {
            continue;
        }
        let Some(snapshot) = plan.read(&path) else { continue };
        let Some(bytes) = snapshot.bytes() else { continue };
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(bytes) else { continue };
        let Some(target) = value.get("target_executable").and_then(|value| value.as_str()).map(PathBuf::from) else {
            continue;
        };
        if !target.is_absolute() || protected(&target, places, old_root) {
            continue;
        }
        let standard_name = target
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| ["fastctx", "fastctx.exe"].contains(&name));
        if standard_name && files::check_path(&target).is_ok() && processes::classify(&target).unwrap_or(false) {
            targets.push(target);
        }
    }
    targets.sort();
    targets.dedup();
    for target in targets {
        if protected(&target, places, old_root) {
            continue;
        }
        let Some(parent) = target.parent() else { continue };
        let Some(basename) = target.file_name().and_then(|name| name.to_str()) else { continue };
        for path in super::entries(parent, plan) {
            let name = path.file_name().and_then(|name| name.to_str()).unwrap_or("");
            let Some(writer) = replacement_with_rf(name, basename) else { continue };
            if let Some(pid) = writer
                && !plan.creator_ended(&path, pid)
            {
                continue;
            }
            if !plan.quiet(&path, false) || !plan.unused(&path, true) {
                continue;
            }
            let blockers = match plan.inventory() {
                Ok(inventory) => inventory
                    .processes
                    .iter()
                    .filter(|process| process.pid != std::process::id() && processes::within(&process.image, &target))
                    .map(|process| {
                        format!("PID {} still uses the replaced executable; sibling retirement deferred", process.pid)
                    })
                    .collect::<Vec<_>>(),
                Err(error) => vec![error.into()],
            };
            if !blockers.is_empty() {
                for reason in blockers {
                    plan.active(&path, reason);
                }
                continue;
            }
            let regular = std::fs::symlink_metadata(&path)
                .is_ok_and(|metadata| metadata.is_file() && !crate::disk::is_link(&metadata));
            if regular {
                match processes::classify(&path) {
                    Ok(true) => {
                        if let Some(snapshot) = plan.read(&path)
                            && files::plain(&snapshot)
                        {
                            plan.delete(snapshot, "unchanged published executable in an abandoned replacement slot");
                            continue;
                        }
                    }
                    Ok(false) => {}
                    Err(error) => {
                        plan.active(&path, error.to_string());
                        continue;
                    }
                }
            }
            plan.archive(
                &path,
                "executable-replacements",
                "replacement residue retained unchanged, including modifications",
            );
        }
    }
}

fn protected(target: &Path, places: &Places, old_root: &Path) -> bool {
    processes::within(target, &old_root.join("bin")) || processes::within(target, &places.root.join("bin"))
}

pub(super) fn replacement(name: &str, basename: &str) -> Option<Option<u32>> {
    if let Some(rest) = name.strip_prefix(&format!(".{basename}.fastctx-old-")) {
        return pair(rest).map(Some);
    }
    if let Some(rest) = name.strip_prefix(&format!(".{basename}."))?.strip_suffix(".tmp")
        && let Some(pid) = pair(rest)
    {
        return Some(Some(pid));
    }
    None
}

pub(super) fn replacement_with_rf(name: &str, basename: &str) -> Option<Option<u32>> {
    replacement(name, basename).or_else(|| {
        let lower = name.to_ascii_lowercase();
        (lower.starts_with(&format!("{}~rf", basename.to_ascii_lowercase())) && lower.ends_with(".tmp")).then_some(None)
    })
}

fn pair(value: &str) -> Option<u32> {
    let (pid, sequence) = value.split_once('.')?;
    if !pid.bytes().all(|byte| byte.is_ascii_digit()) || !sequence.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    sequence.parse::<u64>().ok()?;
    pid.parse::<u32>().ok().filter(|pid| *pid != 0)
}

pub(super) fn writers_ended(directory: &Path, plan: &mut Plan) -> bool {
    let mut ended = true;
    for path in super::entries(directory, plan) {
        let name = path.file_name().and_then(|name| name.to_str()).unwrap_or("");
        for basename in ["config.toml", "fastread", "fastread.exe", "fastctx", "fastctx.exe"] {
            if let Some(Some(pid)) = replacement_with_rf(name, basename) {
                ended &= plan.creator_ended(&path, pid);
            }
        }
    }
    let bin = directory.join("bin");
    if bin.is_dir() {
        for path in super::entries(&bin, plan) {
            let name = path.file_name().and_then(|name| name.to_str()).unwrap_or("");
            for basename in ["fastread", "fastread.exe", "fastctx", "fastctx.exe"] {
                let normalized = name.replace(".fastread-old-", ".fastctx-old-");
                if let Some(Some(pid)) = replacement_with_rf(&normalized, basename) {
                    ended &= plan.creator_ended(&path, pid);
                }
            }
        }
    }
    ended
}
