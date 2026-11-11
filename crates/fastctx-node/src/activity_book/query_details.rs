//! Read the selected deeds' detail sets in batches.

use std::collections::HashMap;

use fastctx_storage::{
    Result,
    rusqlite::{Connection, params_from_iter},
};
use fastctx_types::{
    MachineName,
    activity_book::{Deed, TouchHow, Touched},
};

pub(super) fn load(db: &Connection, deeds: &mut [(String, Deed)]) -> Result<()> {
    if deeds.is_empty() {
        return Ok(());
    }
    let keys: Vec<String> = deeds.iter().map(|(key, _)| key.clone()).collect();
    let marks = (1..=keys.len()).map(|index| format!("?{index}")).collect::<Vec<_>>().join(", ");
    let mut touches: HashMap<String, Vec<Touched>> = HashMap::new();
    let mut statement = db.prepare(&format!(
        "SELECT deed, path, how, added, removed, seen FROM book_touches WHERE deed IN ({marks}) ORDER BY path"
    ))?;
    let rows = statement.query_map(params_from_iter(keys.iter()), |row| {
        Ok((
            row.get::<_, String>(0)?,
            Touched {
                path: row.get(1)?,
                how: how(&row.get::<_, String>(2)?),
                added: row.get::<_, Option<i64>>(3)?.map(|value| value.max(0) as u32),
                removed: row.get::<_, Option<i64>>(4)?.map(|value| value.max(0) as u32),
                seen: row.get(5)?,
            },
        ))
    })?;
    for row in rows {
        let (key, value) = row?;
        touches.entry(key).or_default().push(value);
    }
    let mut visits: HashMap<String, Vec<MachineName>> = HashMap::new();
    let mut statement =
        db.prepare(&format!("SELECT deed, machine FROM book_visits WHERE deed IN ({marks}) ORDER BY deed, machine"))?;
    let rows = statement
        .query_map(params_from_iter(keys.iter()), |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?;
    for row in rows {
        let (key, value) = row?;
        if let Ok(machine) = value.parse() {
            visits.entry(key).or_default().push(machine);
        }
    }
    for (key, deed) in deeds {
        deed.touched = touches.remove(key.as_str()).unwrap_or_default();
        deed.went = visits.remove(key.as_str()).unwrap_or_default();
    }
    Ok(())
}

fn how(word: &str) -> TouchHow {
    match word {
        "added" => TouchHow::Added,
        "removed" => TouchHow::Removed,
        "copied" => TouchHow::Copied,
        _ => TouchHow::Changed,
    }
}
