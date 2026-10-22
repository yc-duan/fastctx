//! Read publications from whole signed entries, including after a crash
//! between keeping an entry and projecting its separate files.

use std::collections::BTreeMap;
use std::io;

use fastctx_types::layout::WorldDir;
use fastctx_types::pages::{
    PageConsent, PageIdentity, PageSummary, PageVersion, PageView, account_consent_path, html_path, version_path,
};
use fastctx_types::{PageName, TownName};

#[derive(Default)]
pub struct Page {
    pub versions: BTreeMap<u32, (PageVersion, Option<String>)>,
    pub consents: BTreeMap<PageIdentity, (PageConsent, fastctx_types::MachineName)>,
}

/// Every page of `town`, from its entries. A file that is not a well-formed
/// version or consent is not one: anyone who may write in the project could
/// have put it there, and it does not take the other pages down with it.
pub fn all(world: &WorldDir, town: &TownName) -> io::Result<BTreeMap<PageName, Page>> {
    let mut pages = BTreeMap::<PageName, Page>::new();
    for (_, entry) in crate::ledger::entries_after(world, town, 0)? {
        for file in &entry.files {
            let parts: Vec<_> = file.path.split('/').collect();
            match parts.as_slice() {
                ["pages", name, "versions", _, "page.json"] => {
                    let Ok(name) = name.parse::<PageName>() else { continue };
                    let Ok(version) = serde_json::from_slice::<PageVersion>(&file.bytes.0) else { continue };
                    if file.path != version_path(&name, version.version) {
                        continue;
                    }
                    let html = entry.files.iter().find(|f| f.path == html_path(&name, version.version));
                    let html = match (version.withdrawn, html) {
                        (false, Some(html)) => match String::from_utf8(html.bytes.0.clone()) {
                            Ok(html) => Some(html),
                            Err(_) => continue,
                        },
                        (true, None) => None,
                        _ => continue,
                    };
                    pages.entry(name).or_default().versions.insert(version.version, (version, html));
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
                        page.consents.insert(identity, (consent, entry.author.clone()));
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

    pub fn view(
        &self,
        town: &TownName,
        page: &PageName,
        version: Option<u32>,
        identity: Option<&PageIdentity>,
    ) -> Result<PageView, String> {
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
            consent: identity.and_then(|identity| self.consents.get(identity)).map(|(consent, _)| consent.clone()),
        })
    }
}
