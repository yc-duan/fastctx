//! Read project memory from verified, immutable ledger entries.

use std::io;

use fastctx_net::ledger::File;
use fastctx_net::memory::Index;
use fastctx_types::TownName;
use fastctx_types::layout::WorldDir;

pub(crate) struct Store {
    pub index: Index,
    pub files: Vec<File>,
}

pub(crate) fn catalog(world: &WorldDir, town: &TownName) -> io::Result<fastctx_types::memory::Catalog> {
    if !world.town(town).signed().is_dir()
        && let Some(index) = crate::ledger::metadata::read(world, town)?
    {
        return Ok(index.catalog);
    }
    type Cached = std::collections::HashMap<
        std::path::PathBuf,
        (crate::ledger::names::Place, u64, std::sync::Arc<fastctx_types::memory::Catalog>),
    >;
    static CATALOGS: std::sync::OnceLock<std::sync::Mutex<Cached>> = std::sync::OnceLock::new();
    let cache = CATALOGS.get_or_init(Default::default);
    let identity = crate::ledger::names::place(world, town)?
        .ok_or_else(|| io::Error::other(fastctx_types::memory::words::MISSING_IDENTITY))?;
    let seq = crate::ledger::replica_seq(world, town)?;
    let path = world.town(town).path().to_path_buf();
    let held = cache
        .lock()
        .expect("never poisoned")
        .get(&path)
        .filter(|(place, version, _)| *place == identity && *version == seq)
        .map(|(_, _, catalog)| catalog.clone());
    if let Some(catalog) = held {
        return Ok((*catalog).clone());
    }
    // Signature verification and disk reads stay outside the shared metadata cache.
    let catalog = read(world, town)?.index.catalog();
    cache.lock().expect("never poisoned").insert(path, (identity, seq, std::sync::Arc::new(catalog.clone())));
    Ok(catalog)
}

pub(crate) fn read(world: &WorldDir, town: &TownName) -> io::Result<Store> {
    let mut index = Index::default();
    let mut files = Vec::new();
    for (_, entry) in crate::ledger::entries_after(world, town, 0)? {
        index.take(&entry).map_err(io::Error::other)?;
        files.extend(entry.files);
    }
    Ok(Store { index, files })
}
