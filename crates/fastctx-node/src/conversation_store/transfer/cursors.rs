//! Typed primary-key cursors for resumable batches and their rollback journal.
use fastctx_storage::{Error, Result, rusqlite::types::Value};

#[derive(serde::Serialize, serde::Deserialize)]
enum Cell {
    Null,
    Integer(i64),
    Real(u64),
    Text(String),
    Blob(Vec<u8>),
}
pub(super) fn encode(values: &[Value]) -> Result<Vec<u8>> {
    let cells: Vec<_> = values
        .iter()
        .map(|value| match value {
            Value::Null => Cell::Null,
            Value::Integer(value) => Cell::Integer(*value),
            Value::Real(value) => Cell::Real(value.to_bits()),
            Value::Text(value) => Cell::Text(value.clone()),
            Value::Blob(value) => Cell::Blob(value.clone()),
        })
        .collect();
    serde_json::to_vec(&cells).map_err(|error| Error::new(error.to_string()))
}
pub(super) fn decode(bytes: &[u8]) -> Result<Vec<Value>> {
    let cells: Vec<Cell> = serde_json::from_slice(bytes).map_err(|error| Error::new(error.to_string()))?;
    Ok(cells
        .into_iter()
        .map(|cell| match cell {
            Cell::Null => Value::Null,
            Cell::Integer(value) => Value::Integer(value),
            Cell::Real(value) => Value::Real(f64::from_bits(value)),
            Cell::Text(value) => Value::Text(value),
            Cell::Blob(value) => Value::Blob(value),
        })
        .collect())
}
