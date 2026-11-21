//! Which conversation each Claude session's directory names, kept in
//! memory. A directory's `resident.json` is written once and never changed
//! (`pairing`), so one read to name a conversation is not read again. The
//! directories are listed again when the directory holding them changed,
//! and at least every `LISTED_FOR`; those naming none yet are read again
//! then. Asked without this, every question of one conversation's sessions
//! read every session's directory on the machine.

use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

use fastctx_types::ResidentId;

/// How long a listing stands while the directory holding the sessions'
/// directories looks unchanged.
const LISTED_FOR: Duration = Duration::from_secs(30);
/// A directory changed this lately may change again within the grain of its
/// clock without looking changed: a listing of it does not stand.
const SETTLED_AFTER: Duration = Duration::from_secs(2);

#[derive(Default)]
pub(super) struct Aliases(Mutex<Kept>);

#[derive(Default)]
struct Kept {
    /// When the directories were last listed, and when the directory
    /// holding them had last changed then; none where it had just changed.
    listed: Option<(Instant, Option<SystemTime>)>,
    /// The directories that name a conversation, by name.
    named: HashMap<String, ResidentId>,
    /// The directories that could not be read at the last listing.
    problems: Vec<String>,
    /// Why they could not be listed at all, where they could not.
    unlisted: Option<String>,
}

impl Aliases {
    /// The directories under `root` that name `resident`, and what could
    /// not be read of any.
    pub(super) fn of(&self, root: &Path, resident: &ResidentId) -> (Vec<PathBuf>, Vec<String>) {
        let mut kept = self.0.lock().expect("never poisoned");
        let changed = fastctx_platform::disk::metadata(root).and_then(|metadata| metadata.modified()).ok();
        let stands = kept
            .listed
            .is_some_and(|(at, was)| was.is_some() && was == changed && at.elapsed() < LISTED_FOR);
        if !stands {
            kept.list(root, changed);
        }
        let mut directories: Vec<PathBuf> =
            kept.named.iter().filter(|(_, named)| *named == resident).map(|(name, _)| root.join(name)).collect();
        directories.sort();
        let mut problems = kept.problems.clone();
        if let Some(error) = &kept.unlisted {
            problems.push(format!("reading {resident}'s Claude sessions: {error}"));
        }
        (directories, problems)
    }

    /// `session`'s directory was just written to name `resident`.
    pub(super) fn named(&self, session: &str, resident: &ResidentId) {
        self.0.lock().expect("never poisoned").named.insert(session.to_string(), resident.clone());
    }
}

impl Kept {
    fn list(&mut self, root: &Path, changed: Option<SystemTime>) {
        let settled = changed
            .filter(|changed| SystemTime::now().duration_since(*changed).is_ok_and(|since| since >= SETTLED_AFTER));
        self.listed = Some((Instant::now(), settled));
        self.problems.clear();
        self.unlisted = None;
        let entries = match fastctx_platform::disk::read_dir(root) {
            Ok(entries) => entries,
            Err(error) => {
                self.named.clear();
                self.unlisted = Some(error.to_string());
                return;
            }
        };
        let mut present = HashSet::new();
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    self.problems.push(format!("reading a Claude session directory: {error}"));
                    continue;
                }
            };
            let directory = entry.path();
            let Some(name) = entry.file_name().to_str().map(str::to_string) else { continue };
            if !self.named.contains_key(&name) {
                match fastctx_platform::disk::read(directory.join("resident.json")) {
                    Ok(bytes) => match serde_json::from_slice::<ResidentId>(&bytes) {
                        Ok(resident) => {
                            self.named.insert(name.clone(), resident);
                        }
                        Err(error) => self.problems.push(format!("reading {}: {error}", directory.display())),
                    },
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => self.problems.push(format!("reading {}: {error}", directory.display())),
                }
            }
            present.insert(name);
        }
        self.named.retain(|name, _| present.contains(name));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resident(name: &str) -> ResidentId {
        name.parse().expect("a resident id")
    }

    fn name_it(root: &Path, session: &str, id: &ResidentId) {
        fastctx_platform::disk::create_dir_all(root.join(session)).expect("directory");
        fastctx_platform::disk::write(root.join(session).join("resident.json"), serde_json::to_vec(id).expect("json")).expect("alias");
    }

    #[test]
    fn a_conversations_sessions_are_found_and_kept() {
        let root = tempfile::tempdir().expect("root");
        let (one, two) = (resident("claude-0a1b2c3d"), resident("claude-4e5f6a7b"));
        name_it(root.path(), "s1", &one);
        name_it(root.path(), "s2", &two);
        fastctx_platform::disk::create_dir_all(root.path().join("unnamed")).expect("directory");
        let aliases = Aliases::default();
        let (found, problems) = aliases.of(root.path(), &one);
        assert_eq!(found, vec![root.path().join("s1")]);
        assert!(problems.is_empty(), "{problems:?}");
        // One added is found at the next asking: the directory holding them
        // changed, or changed too lately for its listing to stand.
        name_it(root.path(), "s3", &one);
        let (found, _) = aliases.of(root.path(), &one);
        assert_eq!(found, vec![root.path().join("s1"), root.path().join("s3")]);
        // One named where its directory was already there is told.
        aliases.named("unnamed", &two);
        let (found, _) = aliases.of(root.path(), &two);
        assert_eq!(found.len(), 2);
        // One gone is found gone.
        fastctx_platform::disk::remove_dir_all(root.path().join("s1")).expect("remove");
        let (found, _) = aliases.of(root.path(), &one);
        assert_eq!(found, vec![root.path().join("s3")]);
    }

    #[test]
    fn a_listing_that_stands_reads_no_directory_again() {
        let root = tempfile::tempdir().expect("root");
        let one = resident("claude-0a1b2c3d");
        name_it(root.path(), "s1", &one);
        let aliases = Aliases::default();
        // Listed as of long enough ago that its listing stands.
        {
            let mut kept = aliases.0.lock().expect("kept");
            let changed = fastctx_platform::disk::metadata(root.path()).and_then(|metadata| metadata.modified()).ok();
            kept.list(root.path(), changed);
            kept.listed = Some((Instant::now(), changed));
        }
        // What names it changes behind its back, its directory untouched:
        // not read again while the listing stands.
        fastctx_platform::disk::write(root.path().join("s1").join("resident.json"), b"not json").expect("spoil");
        let (found, problems) = aliases.of(root.path(), &one);
        assert_eq!(found, vec![root.path().join("s1")]);
        assert!(problems.is_empty(), "{problems:?}");
    }

    #[test]
    fn no_directory_of_sessions_is_said() {
        let root = tempfile::tempdir().expect("root");
        let aliases = Aliases::default();
        let (found, problems) = aliases.of(&root.path().join("none"), &resident("claude-0a1b2c3d"));
        assert!(found.is_empty());
        assert_eq!(problems.len(), 1);
    }
}
