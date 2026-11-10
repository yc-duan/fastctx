//! Compares persisted audit payloads and recipient indexes, apart from clock time.
use crate::Store;
use serde_json::{Value, json};

pub(super) fn read(store: &Store, expected: usize) -> anyhow::Result<Value> {
    let records: Vec<Vec<u8>> = store.shelf.0.read_now(|connection| {
        let mut statement = connection.prepare("SELECT record FROM machine_log ORDER BY id")?;
        let rows = statement.query_map([], |row| row.get(0))?.collect::<Result<_, _>>()?;
        Ok(rows)
    })?;
    anyhow::ensure!(records.len() == expected, "exactly one audit per successful management edit");
    let records = records
        .into_iter()
        .map(|bytes| {
            let mut value: Value = serde_json::from_slice(&bytes)?;
            value["record"]["at"] = Value::Null;
            Ok(value)
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let indexes = ["machine_log_readers", "machine_log_notified"]
        .into_iter()
        .map(|table| {
            let rows: Vec<(String, i64)> = store.shelf.0.read_now(|connection| {
                let mut statement = connection.prepare(&format!("SELECT reader,id FROM {table} ORDER BY reader,id"))?;
                let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?.collect::<Result<_, _>>()?;
                Ok(rows)
            })?;
            Ok((table, rows))
        })
        .collect::<anyhow::Result<std::collections::BTreeMap<_, _>>>()?;
    Ok(json!({"records": records, "indexes": indexes}))
}
