//! Deterministic metadata bytes shared by feed and acceptance commitments.

use serde::Serialize;
use serde_json::Value;

pub(super) fn encode(input: &impl Serialize) -> Result<Vec<u8>, String> {
    let input = serde_json::to_value(input).map_err(|error| error.to_string())?;
    let mut output = Vec::new();
    value(&input, &mut output)?;
    Ok(output)
}

fn value(value: &Value, output: &mut Vec<u8>) -> Result<(), String> {
    match value {
        Value::Object(fields) => {
            output.push(b'{');
            let mut fields: Vec<_> = fields.iter().collect();
            fields.sort_unstable_by(|(left, _), (right, _)| left.as_bytes().cmp(right.as_bytes()));
            for (index, (key, value)) in fields.into_iter().enumerate() {
                if index > 0 {
                    output.push(b',');
                }
                serde_json::to_writer(&mut *output, key).map_err(|error| error.to_string())?;
                output.push(b':');
                self::value(value, output)?;
            }
            output.push(b'}');
        }
        Value::Array(items) => {
            output.push(b'[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    output.push(b',');
                }
                self::value(item, output)?;
            }
            output.push(b']');
        }
        Value::Number(number) if !number.is_i64() && !number.is_u64() => {
            return Err("feed canonicalization excludes floating-point numbers".into());
        }
        _ => serde_json::to_writer(output, value).map_err(|error| error.to_string())?,
    }
    Ok(())
}
