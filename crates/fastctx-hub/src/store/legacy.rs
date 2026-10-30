//! The older directory upgrade is applied inside the import transaction,
//! after the raw source rows have all been read back and compared.
use super::shelf;
use fastctx_storage::{
    Error,
    rusqlite::{Connection, params},
};
use serde_json::{Map, Value};

pub(super) fn normalize(c: &Connection) -> fastctx_storage::Result<()> {
    let mut object = Map::new();
    for kind in ["accounts", "members", "shares", "lends", "plans", "acting", "projects"] {
        let mut stmt = c.prepare_cached("SELECT body FROM hub_rows WHERE kind=?1 ORDER BY position,key")?;
        let mut cursor = stmt.query([kind])?;
        let mut rows = Vec::new();
        while let Some(row) = cursor.next()? {
            rows.push(shelf::decode::<Value>(&row.get::<_, Vec<u8>>(0)?)?);
        }
        object.insert(kind.into(), Value::Array(rows));
    }
    for field in ["grants", "trust"] {
        if let Some(value) = shelf::meta(c, &format!("book.{field}"))? {
            object.insert(field.into(), value);
        }
    }
    let (mut normalized, mut said) = crate::upgrade::upgrade(Value::Object(object));
    said.extend(crate::upgrade::drop_relays(&mut normalized));
    let _: fastctx_net::directory::Directory =
        serde_json::from_value(normalized.clone()).map_err(|e| Error::new(e.to_string()))?;
    if said.is_empty() {
        return Ok(());
    }
    for kind in ["accounts", "members", "shares", "lends", "plans", "acting", "projects"] {
        c.prepare_cached("DELETE FROM hub_rows WHERE kind=?1")?.execute([kind])?;
        let mut seen = std::collections::HashMap::<String, usize>::new();
        for (position, value) in
            normalized[kind].as_array().ok_or_else(|| Error::new("invalid upgraded directory"))?.iter().enumerate()
        {
            let base = super::migrate::key(kind, value);
            let occurrence = seen.entry(base.clone()).or_default();
            *occurrence += 1;
            let key = if *occurrence == 1 { base } else { format!("{base}#{occurrence}") };
            let bytes = serde_json::to_vec(value).map_err(|e| Error::new(e.to_string()))?;
            shelf::put(c, kind, &key, position as i64, &bytes)?;
            let read: Vec<u8> = c
                .prepare_cached("SELECT body FROM hub_rows WHERE kind=?1 AND key=?2")?
                .query_row(params![kind, key], |r| r.get(0))?;
            if read != bytes {
                return Err(Error::new("an upgraded directory row did not read back"));
            }
        }
    }
    shelf::put_meta(c, "upgraded", &serde_json::to_vec(&said).map_err(|e| Error::new(e.to_string()))?)
}
