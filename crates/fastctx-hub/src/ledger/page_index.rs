//! Existing page summaries included in the direct project index, without HTML.

use fastctx_types::{
    PageName,
    access::{HubId, TownId},
    pages::{PageSummary, PageVersion, html_path, version_path},
};
use std::{collections::BTreeMap, path::PathBuf};

pub(super) fn read(paths: Vec<(String, PathBuf)>, hub: &HubId, town: &TownId) -> Result<Vec<PageSummary>, String> {
    let mut pages = BTreeMap::<PageName, PageVersion>::new();
    for (file, path) in paths {
        let parts: Vec<_> = file.split('/').collect();
        let ["pages", name, "versions", _, "page.json"] = parts.as_slice() else { continue };
        let Ok(name) = name.parse::<PageName>() else { continue };
        let signed = super::reading::entry(&path).map_err(|error| error.to_string())?;
        let entry = fastctx_net::ledger::open(&signed, hub, town)?;
        let Some(metadata) = entry.files.iter().find(|part| part.path == file) else { continue };
        let Ok(version) = serde_json::from_slice::<PageVersion>(&metadata.bytes.0) else { continue };
        if file != version_path(&name, version.version) {
            continue;
        }
        let html = entry.files.iter().find(|part| part.path == html_path(&name, version.version));
        let valid = match (version.withdrawn, html) {
            (false, Some(html)) => std::str::from_utf8(&html.bytes.0).is_ok(),
            (true, None) => true,
            _ => false,
        };
        if valid && pages.get(&name).is_none_or(|previous| previous.version < version.version) {
            pages.insert(name, version);
        }
    }
    let mut pages: Vec<_> = pages.into_iter().map(|(page, current)| PageSummary { page, current }).collect();
    pages.sort_by(|a, b| b.current.at.cmp(&a.current.at).then_with(|| a.page.cmp(&b.page)));
    Ok(pages)
}
