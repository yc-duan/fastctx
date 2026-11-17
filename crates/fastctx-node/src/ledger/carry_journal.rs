//! Carry discovery includes journals whose ledger files have already moved.

use super::*;

pub(super) fn keep(world: &WorldDir, town: &TownName, journal: &Journal) -> io::Result<()> {
    crate::record_migration::archive::keep_import(&path(world, town), journal)
}

pub(crate) fn towns(world: &WorldDir) -> io::Result<Vec<TownName>> {
    let mut towns: std::collections::BTreeSet<_> = world.towns()?.into_iter().collect();
    for path in crate::record_migration::archive::import_journals(&world.path().join("towns"), "ledger-import.json")? {
        if let Some(town) = path.parent().and_then(Path::file_name).and_then(|name| name.to_str())
            && let Ok(town) = TownName::try_from(town)
        {
            towns.insert(town);
        }
    }
    Ok(towns.into_iter().collect())
}
