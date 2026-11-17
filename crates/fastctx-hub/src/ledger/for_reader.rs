//! A project's ledger as one reader may know it (§29, 2.4): of the machines
//! an entry names, only those the reader may work on, as the first version's
//! filters tell them (`for_reader` on records, pages, consent, memory and
//! history). What those filters leave alone — the machines a page's acts
//! name, a consent's local identity, the journals of the earliest format —
//! is left alone here too.

use std::collections::BTreeSet;

use fastctx_net::directory::Directory;
use fastctx_net::ledger::{Entry, File, Projection};
use fastctx_net::sign::Bytes;
use fastctx_types::history::HistoryRevision;
use fastctx_types::memory::{Author, Evidence, Kind, ProjectIndex, Revision, markdown};
use fastctx_types::project::Record;
use fastctx_types::{AccountName, MachineName};
use serde::{Serialize, de::DeserializeOwned};

/// The machines `account` may work on, by every name each had, and the
/// machine `from` that asks for it, if any.
pub(crate) fn usable(
    directory: &Directory,
    account: &AccountName,
    from: Option<&MachineName>,
) -> BTreeSet<MachineName> {
    fastctx_net::ledger::reader_machines(directory, account, from)
}

/// What a projection was made by: the machines it took as usable.
pub(crate) fn digest(usable: &BTreeSet<MachineName>) -> Vec<u8> {
    fastctx_net::ledger::reader_digest(usable)
}

/// The entry `entry`, numbered `seq`, as one reads it who may work on the
/// machines `usable` admits; `history` tells a history revision as the hub
/// tells that reader (`history_projection`).
pub(crate) fn entry(
    seq: u64,
    entry: &Entry,
    usable: &dyn Fn(&MachineName) -> bool,
    history: &dyn Fn(&mut HistoryRevision),
) -> Projection {
    // An earlier release's pages are told to no reader any more (beta.18).
    let mut files: Vec<File> = entry.files.iter().filter(|file| !file.path.starts_with("pages/")).cloned().collect();
    filter(&mut files, usable, history);
    Projection {
        seq,
        at: entry.at,
        author: Some(entry.author.clone()).filter(|author| usable(author)),
        machines: entry
            .machines
            .as_ref()
            .map(|machines| machines.iter().filter(|machine| usable(machine)).cloned().collect()),
        files,
    }
}

/// The files of an entry, filtered in place; whether any changed.
pub(crate) fn filter(
    files: &mut [File],
    usable: &dyn Fn(&MachineName) -> bool,
    history: &dyn Fn(&mut HistoryRevision),
) -> bool {
    let mut changed = false;
    // A journal's text names its author in its header: told again where
    // the author is told otherwise (`memory::Document::for_reader`).
    let mut attributed: Vec<(String, Author)> = Vec::new();
    for file in files.iter_mut() {
        let parts: Vec<&str> = file.path.split('/').collect();
        let bytes = &file.bytes.0;
        let rewritten = match parts.as_slice() {
            ["project", name] if name.ends_with(".json") => {
                rewrite(bytes, |record: &mut Record| record.for_reader(usable))
            }
            ["memory", _, name] if name.ends_with(".json") => rewrite(bytes, |revision: &mut Revision| {
                let written = revision.author.clone();
                revision.author.for_reader(usable);
                if revision.kind == Kind::Journal && revision.author != written {
                    attributed.push((revision.path("md"), revision.author.clone()));
                }
            }),
            // Evidence may be named under folders of its own: every depth.
            ["memory", _, "evidence", .., name] if name.ends_with(".json") => {
                rewrite(bytes, |evidence: &mut Evidence| {
                    evidence.author.for_reader(usable);
                    if evidence.location.as_ref().is_some_and(|location| !usable(&location.machine)) {
                        evidence.location = None;
                    }
                })
            }
            ["history", _, name] if name.ends_with(".json") => {
                rewrite(bytes, |revision: &mut fastctx_net::topic_history::Revision| history(&mut revision.revision))
            }
            _ => None,
        };
        if let Some(bytes) = rewritten {
            file.bytes = Bytes(bytes);
            changed = true;
        }
    }
    for (path, author) in attributed {
        if let Some(file) = files.iter_mut().find(|file| file.path == path)
            && let Ok(text) = std::str::from_utf8(&file.bytes.0)
        {
            file.bytes = Bytes(markdown::attributed(text, &author).into_bytes());
        }
    }
    changed
}

/// The file `bytes` as `filter` leaves its value, when it changes it; a
/// file that is not such a value is left as it is.
fn rewrite<T: Serialize + DeserializeOwned + PartialEq + Clone>(
    bytes: &[u8],
    filter: impl FnOnce(&mut T),
) -> Option<Vec<u8>> {
    let mut value: T = serde_json::from_slice(bytes).ok()?;
    let written = value.clone();
    filter(&mut value);
    if value == written { None } else { serde_json::to_vec(&value).ok() }
}

/// A project's index as the asking machine's account may know it: its
/// record, opening and catalog (`ProjectIndex`, M09).
pub(crate) fn index(index: &mut ProjectIndex, usable: &dyn Fn(&MachineName) -> bool) {
    index.record.for_reader(usable);
    index.opened.for_reader(usable);
    index.catalog.for_reader(usable);
}

/// A memory document's versions and evidence as the asking machine's
/// account may know them.
pub(crate) fn document(versions: &mut [Revision], evidence: &mut [Evidence], usable: &dyn Fn(&MachineName) -> bool) {
    for revision in versions {
        revision.author.for_reader(usable);
    }
    for evidence in evidence {
        evidence.author.for_reader(usable);
        if evidence.location.as_ref().is_some_and(|location| !usable(&location.machine)) {
            evidence.location = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fastctx_types::project::{Stamp, Writer};

    fn machine(name: &str) -> MachineName {
        MachineName::try_from(name.to_string()).expect("a machine name")
    }

    fn revision(on: &str) -> Revision {
        Revision {
            kind: Kind::Spec,
            name: "plan".into(),
            description: String::new(),
            version: 1,
            author: Author {
                stamp: Stamp { by: Writer::User, on: Some(machine(on)), at: jiff::Timestamp::UNIX_EPOCH },
                host: None,
                model: None,
                session: Some("s".into()),
            },
        }
    }

    #[test]
    fn a_file_is_told_as_the_first_version_tells_it() {
        let written = revision("m2");
        let mut files = vec![
            File { path: written.path("json"), bytes: Bytes(serde_json::to_vec(&written).unwrap()) },
            File { path: written.path("md"), bytes: Bytes(b"text".to_vec()) },
        ];
        let all = |_: &MachineName| true;
        let only_m1 = |name: &MachineName| name.as_str() == "m1";
        let untouched = files.clone();
        assert!(!filter(&mut files, &all, &|_| {}), "one who may work everywhere reads it as written");
        assert_eq!(files, untouched);
        assert!(filter(&mut files, &only_m1, &|_| {}));
        let told: Revision = serde_json::from_slice(&files[0].bytes.0).unwrap();
        let mut expected = written.clone();
        expected.author.for_reader(&only_m1);
        assert_eq!(told, expected, "the same as the first version's filter");
        assert_eq!(told.author.stamp.on, None);
        assert_eq!(files[1], untouched[1], "a spec's text names no author");
        // One who may work nowhere reads it as the first version tells it too.
        let mut nowhere = untouched.clone();
        let none = |_: &MachineName| false;
        assert!(filter(&mut nowhere, &none, &|_| {}));
        let told: Revision = serde_json::from_slice(&nowhere[0].bytes.0).unwrap();
        let mut expected = written.clone();
        expected.author.for_reader(&none);
        assert_eq!(told, expected);
        assert_eq!(told.author.session, None, "nor the session of a writer not named");
    }
}
