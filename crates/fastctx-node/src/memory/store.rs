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

pub(crate) fn read(world: &WorldDir, town: &TownName) -> io::Result<Store> {
    let mut index = Index::default();
    let mut files = Vec::new();
    for (_, entry) in crate::ledger::entries_after(world, town, 0)? {
        index.take(&entry).map_err(io::Error::other)?;
        files.extend(entry.files);
    }
    Ok(Store { index, files })
}
