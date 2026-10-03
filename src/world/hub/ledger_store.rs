//! Ledger sequence, identity, paths, and project membership commit in one redb transaction.
use super::*;
use crate::world::ledger::{Row, SignedEntry, Town};

const LEDGER: TableDefinition<(&str, u64), &[u8]> = TableDefinition::new("ledger");
const IDS: TableDefinition<&str, &[u8]> = TableDefinition::new("ledger_ids");
const PATHS: TableDefinition<(&str, &str), &str> = TableDefinition::new("ledger_paths");
const TOWNS: TableDefinition<&str, &[u8]> = TableDefinition::new("towns");
pub(super) fn initialize(txn: &redb::WriteTransaction) -> Result<(), String> {
    txn.open_table(LEDGER).map_err(|e| e.to_string())?;
    txn.open_table(IDS).map_err(|e| e.to_string())?;
    txn.open_table(PATHS).map_err(|e| e.to_string())?;
    txn.open_table(TOWNS).map_err(|e| e.to_string())?;
    Ok(())
}
impl Store {
    pub(crate) fn append_ledger(&self, signed: &SignedEntry) -> Result<Row, String> {
        let entry = signed.parse()?;
        entry.validate()?;
        self.write(|txn| {
            let mut ledger = txn.open_table(LEDGER).map_err(|e| e.to_string())?;
            let mut ids = txn.open_table(IDS).map_err(|e| e.to_string())?;
            if let Some(previous) = get_json::<Row>(&ids, &entry.id)? {
                return if &previous.signed == signed {
                    Ok(previous)
                } else {
                    Err("ledger id reused with different bytes".into())
                };
            }
            let mut towns = txn.open_table(TOWNS).map_err(|e| e.to_string())?;
            let mut town = get_json::<Town>(&towns, &entry.scope)?.unwrap_or_default();
            if town.seq == 0 && entry.project.is_none() {
                return Err("first ledger entry must contain project attributes".into());
            }
            if town.seq > 0 && !town.machines.contains(&entry.author) {
                return Err("ledger author is outside this town".into());
            }
            if let Some(update) = &entry.project {
                if update.previous != town.project_version {
                    return Err(format!(
                        "project_conflict: current version is {}",
                        town.project_version
                    ));
                }
                town.machines = entry.machines(update)?;
                let members = txn.open_table(MEMBERS).map_err(|e| e.to_string())?;
                for name in &town.machines {
                    if get_json::<MemberRow>(&members, name)?.is_none_or(|row| row.is_revoked()) {
                        return Err(format!(
                            "project names an unknown or revoked member: {name}"
                        ));
                    }
                }
                town.project_version += 1;
            }
            let mut paths = txn.open_table(PATHS).map_err(|e| e.to_string())?;
            for file in &entry.files {
                let key = file.path.to_lowercase();
                for item in paths
                    .range::<(&str, &str)>(
                        (entry.scope.as_str(), "")..=(entry.scope.as_str(), "\u{10ffff}"),
                    )
                    .map_err(|e| e.to_string())?
                {
                    let (existing, _) = item.map_err(|e| e.to_string())?;
                    let existing = existing.value().1;
                    if key == existing
                        || key.starts_with(&format!("{existing}/"))
                        || existing.starts_with(&format!("{key}/"))
                    {
                        return Err(format!("ledger path already occupied: {}", file.path));
                    }
                }
            }
            // Also reject file/directory collisions inside this bundle.
            for a in &entry.files {
                for b in &entry.files {
                    if a.path
                        .to_lowercase()
                        .starts_with(&format!("{}/", b.path.to_lowercase()))
                    {
                        return Err("ledger bundle has a file/directory collision".into());
                    }
                }
            }
            town.seq += 1;
            let row = Row {
                seq: town.seq,
                signed: signed.clone(),
            };
            let bytes = encode(&row)?;
            ledger
                .insert((entry.scope.as_str(), row.seq), bytes.as_slice())
                .map_err(|e| e.to_string())?;
            ids.insert(entry.id.as_str(), bytes.as_slice())
                .map_err(|e| e.to_string())?;
            for file in &entry.files {
                paths
                    .insert(
                        (entry.scope.as_str(), file.path.to_lowercase().as_str()),
                        entry.id.as_str(),
                    )
                    .map_err(|e| e.to_string())?;
            }
            towns
                .insert(entry.scope.as_str(), encode(&town)?.as_slice())
                .map_err(|e| e.to_string())?;
            Ok(row)
        })
    }
    pub(crate) fn towns_for(&self, name: &str) -> Result<BTreeMap<String, Town>, String> {
        self.read(|txn| {
            let towns = txn.open_table(TOWNS).map_err(|e| e.to_string())?;
            let mut result = BTreeMap::new();
            for item in towns.iter().map_err(|e| e.to_string())? {
                let (key, value) = item.map_err(|e| e.to_string())?;
                let town: Town = decode(value.value())?;
                if town.machines.iter().any(|member| member == name) {
                    result.insert(key.value().into(), town);
                }
            }
            Ok(result)
        })
    }
    pub(crate) fn ledger_next(
        &self,
        name: &str,
        scope: &str,
        after: u64,
    ) -> Result<Option<Row>, String> {
        self.read(|txn| {
            let towns = txn.open_table(TOWNS).map_err(|e| e.to_string())?;
            let town = get_json::<Town>(&towns, scope)?.ok_or("Unknown town")?;
            if !town.machines.iter().any(|member| member == name) {
                return Err("This member is outside the town".into());
            }
            let ledger = txn.open_table(LEDGER).map_err(|e| e.to_string())?;
            ledger
                .get((scope, after + 1))
                .map_err(|e| e.to_string())?
                .map(|v| decode(v.value()))
                .transpose()
        })
    }
}
