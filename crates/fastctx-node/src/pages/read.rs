//! Read publications from whole signed entries, including after a crash
//! between keeping an entry and projecting its separate files.
//!
//! What the entries say of a project's pages is kept in memory from one
//! read to the next, and only the entries that came since are read: a
//! project's page was read every two seconds while it was looked at, and
//! each read took every entry of its ledger, with every version of every
//! page in it. Only what describes a version is kept; the page itself is
//! read from the one entry that holds it, when that version is asked for.

use std::collections::{BTreeMap, HashMap};
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex};

use fastctx_net::ledger::File;
use fastctx_types::layout::WorldDir;
use fastctx_types::pages::{
    PageConsent, PageIdentity, PageSummary, PageVersion, PageView, account_consent_path, html_path, version_path,
};
use fastctx_types::{MachineName, PageName, TownName};

#[derive(Clone, Default)]
pub struct Page {
    /// Each version, with the number of the entry that holds its page; a
    /// withdrawn version has none.
    pub versions: BTreeMap<u32, (PageVersion, Option<u64>)>,
    pub consents: BTreeMap<PageIdentity, (PageConsent, MachineName)>,
}

/// What a project's entries, up to the one numbered `through`, say of its
/// pages, for the ledger `of`.
#[derive(Default)]
struct Kept {
    of: Option<crate::ledger::names::Place>,
    through: u64,
    pages: BTreeMap<PageName, Page>,
}

static KEPT: LazyLock<Mutex<HashMap<PathBuf, Arc<Mutex<Kept>>>>> = LazyLock::new(Default::default);

/// Reads the pages of `town` as its entries stand now. A file that is not a
/// well-formed version or consent is not one: anyone who may write in the
/// project could have put it there, and it does not take the other pages
/// down with it.
fn with<R>(world: &WorldDir, town: &TownName, read: impl FnOnce(&BTreeMap<PageName, Page>) -> R) -> io::Result<R> {
    let kept = KEPT.lock().expect("never poisoned").entry(world.town(town).path().to_path_buf()).or_default().clone();
    let mut kept = kept.lock().expect("never poisoned");
    // Another ledger under the same name, as after leaving a project and
    // entering one called the same, or one shorter than what was read: what
    // is kept is of a ledger that is gone.
    let of = crate::ledger::names::place(world, town)?;
    let (mut last, mut entries) = crate::ledger::entries_since(world, town, kept.through)?;
    if kept.of != of || last < kept.through {
        *kept = Kept { of, ..Kept::default() };
        (last, entries) = crate::ledger::entries_since(world, town, 0)?;
    }
    for (seq, entry) in entries {
        apply(&mut kept.pages, seq, &entry.author, &entry.files);
    }
    kept.through = last;
    Ok(read(&kept.pages))
}

/// Takes what one entry says of pages.
fn apply(pages: &mut BTreeMap<PageName, Page>, seq: u64, author: &MachineName, files: &[File]) {
    for file in files {
        let parts: Vec<_> = file.path.split('/').collect();
        match parts.as_slice() {
            ["pages", name, "versions", _, "page.json"] => {
                let Ok(name) = name.parse::<PageName>() else { continue };
                let Ok(version) = serde_json::from_slice::<PageVersion>(&file.bytes.0) else { continue };
                if file.path != version_path(&name, version.version) {
                    continue;
                }
                let html = files.iter().find(|f| f.path == html_path(&name, version.version));
                let held = match (version.withdrawn, html) {
                    (false, Some(html)) if std::str::from_utf8(&html.bytes.0).is_ok() => Some(seq),
                    (true, None) => None,
                    _ => continue,
                };
                pages.entry(name).or_default().versions.insert(version.version, (version, held));
            }
            ["pages", name, "consent", _, _] => {
                let Ok(name) = name.parse::<PageName>() else { continue };
                let Ok(consent) = serde_json::from_slice::<PageConsent>(&file.bytes.0) else { continue };
                let Some(identity) = consent.identity.clone() else { continue };
                if file.path != account_consent_path(&name, &identity, consent.version)
                    || consent.stamp.by != fastctx_types::project::Writer::User
                {
                    continue;
                }
                let page = pages.entry(name).or_default();
                if page.consents.get(&identity).is_none_or(|(old, _)| old.version < consent.version) {
                    page.consents.insert(identity, (consent, author.clone()));
                }
            }
            _ => {}
        }
    }
}

/// One page of `town`: its versions and the consents given to it. A page
/// the project does not have has none.
pub fn page(world: &WorldDir, town: &TownName, page: &PageName) -> io::Result<Page> {
    with(world, town, |pages| pages.get(page).cloned().unwrap_or_default())
}

pub fn summaries(world: &WorldDir, town: &TownName) -> io::Result<Vec<PageSummary>> {
    let mut result: Vec<_> = with(world, town, |pages| {
        pages
            .iter()
            .filter_map(|(page, stored)| {
                stored.current().map(|current| PageSummary { page: page.clone(), current: current.clone() })
            })
            .collect()
    })?;
    result.sort_by(|a, b| b.current.at.cmp(&a.current.at).then_with(|| a.page.cmp(&b.page)));
    Ok(result)
}

/// What is said of a page, or a version of one, the project does not have.
pub fn missing(town: &TownName, page: &PageName, version: Option<u32>) -> String {
    format!("{town} has no page {page} at {}", version.map_or("any version".into(), |v| format!("version {v}")))
}

impl Page {
    pub fn current(&self) -> Option<&PageVersion> {
        self.versions.last_key_value().map(|(_, (v, _))| v)
    }

    /// A version of the page as a reader sees it, the page itself read from
    /// the entry that holds it.
    pub fn view(
        &self,
        world: &WorldDir,
        town: &TownName,
        page: &PageName,
        version: Option<u32>,
        identity: Option<&PageIdentity>,
    ) -> Result<PageView, String> {
        let (publication, held) = match version {
            Some(version) => self.versions.get(&version),
            None => self.versions.last_key_value().map(|(_, version)| version),
        }
        .ok_or_else(|| missing(town, page, version))?;
        let html = match held {
            Some(seq) => Some(html(world, town, *seq, page, publication.version)?),
            None => None,
        };
        Ok(PageView {
            town: town.clone(),
            page: page.clone(),
            publication: publication.clone(),
            html,
            consent: identity.and_then(|identity| self.consents.get(identity)).map(|(consent, _)| consent.clone()),
        })
    }
}

/// The page a version published, from the entry numbered `seq`.
fn html(world: &WorldDir, town: &TownName, seq: u64, page: &PageName, version: u32) -> Result<String, String> {
    let gone = || format!("version {version} of {page} is no longer in the entries of {town} this machine has");
    let entry = crate::ledger::entry(world, town, seq).map_err(|error| error.to_string())?.ok_or_else(gone)?;
    let path = html_path(page, version);
    let file = entry.files.into_iter().find(|file| file.path == path).ok_or_else(gone)?;
    String::from_utf8(file.bytes.0).map_err(|_| gone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use fastctx_net::sign::Bytes;
    use fastctx_types::project::Writer;

    fn published(page: &str, version: u32, withdrawn: bool) -> Vec<File> {
        let name: PageName = page.parse().unwrap();
        let said = PageVersion {
            version,
            base: version.checked_sub(1).filter(|base| *base > 0),
            title: format!("{page} {version}"),
            description: String::new(),
            acts: Vec::new(),
            withdrawn,
            restored_from: None,
            by: Writer::User,
            on: None,
            at: jiff::Timestamp::from_second(i64::from(version)).unwrap(),
        };
        let mut files =
            vec![File { path: version_path(&name, version), bytes: Bytes(serde_json::to_vec(&said).unwrap()) }];
        if !withdrawn {
            files.push(File { path: html_path(&name, version), bytes: Bytes(b"<p>page</p>".to_vec()) });
        }
        files
    }

    fn author() -> MachineName {
        "desktop".parse().unwrap()
    }

    #[test]
    fn entries_taken_one_by_one_say_what_they_say_together() {
        let entries = [
            published("plan", 1, false),
            published("notes", 1, false),
            published("plan", 2, false),
            // Not a version: its path names another number than it says.
            vec![File {
                path: "pages/plan/versions/9/page.json".into(),
                bytes: published("plan", 3, false)[0].bytes.clone(),
            }],
            // A version without its page is not one, unless it withdraws.
            vec![published("notes", 2, false).remove(0)],
            published("plan", 3, true),
        ];
        let mut at_once = BTreeMap::new();
        for (index, files) in entries.iter().enumerate() {
            apply(&mut at_once, index as u64 + 1, &author(), files);
        }
        // The same entries, some taken before a read and the rest after it.
        for split in 0..=entries.len() {
            let mut in_two = BTreeMap::new();
            for (index, files) in entries.iter().enumerate().take(split) {
                apply(&mut in_two, index as u64 + 1, &author(), files);
            }
            for (index, files) in entries.iter().enumerate().skip(split) {
                apply(&mut in_two, index as u64 + 1, &author(), files);
            }
            assert_eq!(in_two.keys().collect::<Vec<_>>(), at_once.keys().collect::<Vec<_>>());
        }
        let plan = &at_once[&"plan".parse::<PageName>().unwrap()];
        assert_eq!(plan.versions.keys().copied().collect::<Vec<_>>(), [1, 2, 3]);
        assert_eq!(plan.versions[&1].1, Some(1), "the first version's page is in the first entry");
        assert_eq!(plan.versions[&2].1, Some(3));
        assert_eq!(plan.versions[&3].1, None, "a withdrawn version holds no page");
        assert!(plan.current().unwrap().withdrawn);
        let notes = &at_once[&"notes".parse::<PageName>().unwrap()];
        assert_eq!(notes.versions.keys().copied().collect::<Vec<_>>(), [1]);
    }
}
