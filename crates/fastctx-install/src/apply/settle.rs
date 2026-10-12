//! An apply's recovery record holds the host settings it changes, and the
//! settings can carry a login's token, so it lives only while the change
//! does (`world-run.md`, apply). A change cut short by a crash is finished,
//! forward or back, by the next apply or unapply, and by the node when it
//! starts, whichever comes first; settings copies an earlier version kept go
//! the same way.

use std::path::Path;

use fastctx_platform::FileLock;

/// Finishes the file change an interrupted apply left under `root`, discards
/// the records of settled ones, and removes the settings copies an earlier
/// version kept. Plugin commands an interrupted apply owes are left to the
/// next apply or unapply, which can run Claude Code. Returns what could not
/// be done, each with why; nothing is created when there is nothing to do.
pub fn settle(root: &Path) -> Vec<String> {
    let journal = root.join("applied-files-pending.json");
    let mut problems = Vec::new();
    let owed =
        [journal.as_path(), &root.join("apply-history"), &root.join("claude-command-history")].iter().any(|path| {
            match std::fs::symlink_metadata(path) {
                Ok(_) => true,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
                Err(error) => {
                    problems.push(format!("{}: {error}", path.display()));
                    false
                }
            }
        });
    if !owed {
        return problems;
    }
    let _lock = match FileLock::acquire(&root.join("apply.lock")) {
        Ok(lock) => lock,
        Err(error) => {
            problems.push(format!("taking the apply lock: {error}"));
            return problems;
        }
    };
    match crate::transaction::recover(&journal) {
        Ok(receipt) => {
            problems.extend(receipt.cleanup.iter().map(ToString::to_string));
            problems
                .extend(receipt.preserved.iter().map(|path| format!("conflicting versions kept: {}", path.display())));
        }
        Err(error) => problems.push(error.to_string()),
    }
    if let Err(error) = crate::transaction::collect_finished(root) {
        problems.push(error.to_string());
    }
    problems.extend(crate::claude::history::retire(root));
    problems
}
