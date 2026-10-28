//! Files a script changed in a git repository with programs of its own
//! rather than the World's write commands. After its job ends, each
//! repository the script went into with `cd` is asked which files differ
//! from its last commit; those last modified while the script ran are noted
//! as seen changed, with their line counts against that commit. One
//! repository is asked at a time, briefly; what cannot be asked is left out.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use fastctx_storage::rusqlite::params;
use fastctx_types::run::JobRecord;
use jiff::Timestamp;

use crate::Node;

/// How long one question to git may take.
const ASKING: Duration = Duration::from_secs(10);
/// The most files noted from one repository for one script.
const MOST: usize = 200;
/// Slack around the script's run for file times.
const SLACK: jiff::SignedDuration = jiff::SignedDuration::from_secs(2);

static ONE_AT_A_TIME: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);

pub(super) fn look(node: &Arc<Node>, record: &JobRecord) {
    let Some(ended) = record.end.as_ref().map(|end| end.at) else { return };
    let Ok(home) = fastctx_platform::user_home() else { return };
    let places = places(&record.script, &home);
    if places.is_empty() {
        return;
    }
    let (node, job, began) = (node.clone(), record.id.to_string(), record.started_at);
    tokio::spawn(async move {
        let Ok(_one) = ONE_AT_A_TIME.acquire().await else { return };
        let mut tops = Vec::new();
        for place in places {
            if let Some(top) = git(&place, &["rev-parse", "--show-toplevel"]).await
                && let Some(top) = String::from_utf8(top).ok().map(|top| PathBuf::from(top.trim()))
                && !tops.contains(&top)
            {
                tops.push(top);
            }
        }
        let mut noted = Vec::new();
        for top in tops {
            noted.extend(changed(&top, began, ended).await);
        }
        if noted.is_empty() {
            return;
        }
        let size = noted.iter().map(|(path, ..)| path.len() + 48).sum();
        let result = node
            .database
            .write(size, move |connection| {
                for (path, how, added, removed) in &noted {
                    connection.execute(
                        "INSERT OR IGNORE INTO book_touched (job, path, how, added, removed, seen)
                         VALUES (?1, ?2, ?3, ?4, ?5, 1)",
                        params![job, path, how, added, removed],
                    )?;
                }
                Ok(())
            })
            .await;
        if let Err(error) = result {
            node.log.line(format_args!("noting files a script changed: {error}"));
        }
    });
}

/// The directories a script went into with a plain `cd`: absolute, from
/// the home directory, or relative to the home it starts in.
fn places(script: &str, home: &Path) -> Vec<PathBuf> {
    let mut places = Vec::new();
    for part in script.split([';', '&', '|', '\n', '(', '{']) {
        let part = part.trim_start();
        let part = part.strip_prefix("then ").or_else(|| part.strip_prefix("do ")).unwrap_or(part).trim_start();
        let Some(rest) = part.strip_prefix("cd ") else { continue };
        let target = rest.trim();
        let target = target
            .strip_prefix('"')
            .and_then(|target| target.split('"').next())
            .or_else(|| target.strip_prefix('\'').and_then(|target| target.split('\'').next()))
            .unwrap_or_else(|| target.split_whitespace().next().unwrap_or_default());
        if target.is_empty() || target.contains('$') && !target.starts_with("$HOME") {
            continue;
        }
        let path = if let Some(rest) = target.strip_prefix("~/").or_else(|| target.strip_prefix("$HOME/")) {
            home.join(rest)
        } else if target == "~" || target == "$HOME" {
            continue;
        } else if let Some(windows) = msys_drive(target) {
            windows
        } else if Path::new(target).is_absolute() {
            PathBuf::from(target)
        } else {
            home.join(target)
        };
        if path.is_dir() && !places.contains(&path) {
            places.push(path);
        }
        if places.len() >= 4 {
            break;
        }
    }
    places
}

/// `/c/Users/me` as Git Bash writes `C:/Users/me`, on Windows.
fn msys_drive(target: &str) -> Option<PathBuf> {
    if !cfg!(windows) {
        return None;
    }
    let mut chars = target.chars();
    match (chars.next(), chars.next(), chars.next()) {
        (Some('/'), Some(drive), Some('/')) if drive.is_ascii_alphabetic() => {
            Some(PathBuf::from(format!("{}:/{}", drive.to_ascii_uppercase(), &target[3..])))
        }
        _ => None,
    }
}

/// The files of the repository at `top` that differ from its last commit
/// and were last modified between `began` and `ended`.
async fn changed(
    top: &Path,
    began: Timestamp,
    ended: Timestamp,
) -> Vec<(String, &'static str, Option<u32>, Option<u32>)> {
    let Some(status) = git(top, &["status", "--porcelain=v1", "-z", "--untracked-files=normal"]).await else {
        return Vec::new();
    };
    let (Ok(from), Ok(until)) = (began.checked_sub(SLACK), ended.checked_add(SLACK)) else { return Vec::new() };
    let mut found = Vec::new();
    let mut entries = status.split(|byte| *byte == 0);
    while let Some(entry) = entries.next() {
        if entry.len() < 4 {
            continue;
        }
        let (code, path) = (&entry[..2], String::from_utf8_lossy(&entry[3..]).into_owned());
        if code[0] == b'R' || code[0] == b'C' {
            // A rename names its source next; the change is the new path's.
            entries.next();
        }
        if code.contains(&b'D') {
            continue;
        }
        let full = top.join(&path);
        let Ok(modified) = std::fs::metadata(&full).and_then(|meta| meta.modified()) else { continue };
        let Ok(modified) = Timestamp::try_from(modified) else { continue };
        if modified < from || modified > until {
            continue;
        }
        let how = if code == b"??" || code[0] == b'A' { "added" } else { "changed" };
        found.push((path, how));
        if found.len() >= MOST {
            break;
        }
    }
    if found.is_empty() {
        return Vec::new();
    }
    let mut counts = std::collections::HashMap::new();
    let mut asked = vec!["diff", "--numstat", "-z", "HEAD", "--"];
    asked.extend(found.iter().filter(|(_, how)| *how == "changed").map(|(path, _)| path.as_str()));
    if asked.len() > 5
        && let Some(numstat) = git(top, &asked).await
    {
        for row in numstat.split(|byte| *byte == 0) {
            let row = String::from_utf8_lossy(row);
            let mut parts = row.splitn(3, '\t');
            if let (Some(added), Some(removed), Some(path)) = (parts.next(), parts.next(), parts.next()) {
                counts.insert(path.to_string(), (added.parse().ok(), removed.parse().ok()));
            }
        }
    }
    found
        .into_iter()
        .map(|(path, how)| {
            let (added, removed) = counts.get(&path).copied().unwrap_or((None, None));
            let full = top.join(&path).to_string_lossy().replace('\\', "/");
            (full, how, added, removed)
        })
        .collect()
}

/// What `git -C dir args` printed, when it ran and succeeded in time.
async fn git(dir: &Path, args: &[&str]) -> Option<Vec<u8>> {
    let mut command = tokio::process::Command::new("git");
    command.arg("-C").arg(dir).args(args).stdin(std::process::Stdio::null()).kill_on_drop(true);
    command.env("GIT_OPTIONAL_LOCKS", "0");
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    let output = tokio::time::timeout(ASKING, command.output()).await.ok()?.ok()?;
    output.status.success().then_some(output.stdout)
}
