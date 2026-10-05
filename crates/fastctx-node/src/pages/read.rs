//! Read publications from whole signed entries, including after a crash
//! between keeping an entry and projecting its separate files.

use std::collections::BTreeMap;
use std::io;

use fastctx_types::layout::WorldDir;
use fastctx_types::pages::{PageConsent, PageSummary, PageVersion, PageView, consent_path, html_path, version_path};
use fastctx_types::{PageName, TownName};

#[derive(Default)]
pub struct Page {
    pub versions: BTreeMap<u32, (PageVersion, Option<String>)>,
    pub consent: Option<PageConsent>,
}

pub fn all(world: &WorldDir, town: &TownName) -> io::Result<BTreeMap<PageName, Page>> {
    let mut pages = BTreeMap::<PageName, Page>::new();
    for (_, entry) in crate::ledger::entries_after(world, town, 0)? {
        for file in &entry.files {
            let parts: Vec<_> = file.path.split('/').collect();
            match parts.as_slice() {
                ["pages", name, "versions", _, "page.json"] => {
                    let name: PageName = name.parse().map_err(io::Error::other)?;
                    let version: PageVersion = serde_json::from_slice(&file.bytes.0).map_err(io::Error::other)?;
                    if file.path != version_path(&name, version.version) {
                        return Err(io::Error::other(format!("{} disagrees with its version", file.path)));
                    }
                    let html = entry.files.iter().find(|f| f.path == html_path(&name, version.version));
                    let html = match (version.withdrawn, html) {
                        (false, Some(html)) => Some(String::from_utf8(html.bytes.0.clone()).map_err(io::Error::other)?),
                        (true, None) => None,
                        _ => return Err(io::Error::other(format!("{} has inconsistent HTML", file.path))),
                    };
                    pages.entry(name).or_default().versions.insert(version.version, (version, html));
                }
                ["pages", name, "consent", _] => {
                    let name: PageName = name.parse().map_err(io::Error::other)?;
                    let consent: PageConsent = serde_json::from_slice(&file.bytes.0).map_err(io::Error::other)?;
                    if file.path != consent_path(&name, consent.version)
                        || consent.stamp.by != fastctx_types::project::Writer::User
                    {
                        return Err(io::Error::other(format!("{} is not a user consent record", file.path)));
                    }
                    let page = pages.entry(name).or_default();
                    if page.consent.as_ref().is_none_or(|old| old.version < consent.version) {
                        page.consent = Some(consent);
                    }
                }
                _ => {}
            }
        }
    }
    Ok(pages)
}

pub fn summaries(world: &WorldDir, town: &TownName) -> io::Result<Vec<PageSummary>> {
    let mut result: Vec<_> = all(world, town)?
        .into_iter()
        .filter_map(|(page, mut stored)| {
            stored.versions.pop_last().map(|(_, (current, _))| PageSummary { page, current })
        })
        .collect();
    result.sort_by(|a, b| b.current.at.cmp(&a.current.at).then_with(|| a.page.cmp(&b.page)));
    Ok(result)
}

impl Page {
    pub fn current(&self) -> Option<&PageVersion> {
        self.versions.last_key_value().map(|(_, (v, _))| v)
    }

    pub fn view(&self, town: &TownName, page: &PageName, version: Option<u32>) -> Result<PageView, String> {
        let found = match version {
            Some(version) => self.versions.get(&version),
            None => self.versions.last_key_value().map(|(_, version)| version),
        }
        .ok_or_else(|| {
            format!("{town} has no page {page} at {}", version.map_or("any version".into(), |v| format!("version {v}")))
        })?;
        Ok(PageView {
            town: town.clone(),
            page: page.clone(),
            publication: found.0.clone(),
            html: found.1.clone(),
            consent: self.consent.clone(),
        })
    }
}
