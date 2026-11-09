//! Lossless JSON values keep fields unknown to this release until authorization.
use crate::Error;
use fastctx_sync::store::{authorize::FieldChange, mutation::FieldEdit};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Map, Value};
use std::collections::BTreeSet;

pub(crate) fn object(bytes: &[u8]) -> Result<Map<String, Value>, Error> {
    let value: Value = serde_json::from_slice(bytes).map_err(|_| invalid("invalid JSON object"))?;
    value.as_object().cloned().ok_or_else(|| invalid("asset payload must be an object"))
}
pub(crate) fn typed<T: DeserializeOwned>(value: Value) -> Result<T, Error> {
    serde_json::from_value(value).map_err(|_| invalid("asset fields do not match their schema"))
}
pub(crate) fn bytes(value: &impl Serialize) -> Result<Vec<u8>, Error> {
    serde_json::to_vec(value).map_err(|_| invalid("asset value cannot be encoded"))
}
pub(crate) fn invalid(message: &str) -> Error {
    Error::Invalid(message.into())
}
pub(crate) fn schema(version: u16) -> Result<(), Error> {
    if version == 1 { Ok(()) } else { Err(invalid("unsupported asset schema")) }
}
pub(crate) fn no_version(value: &Map<String, Value>) -> Result<(), Error> {
    if value.contains_key("version") {
        return Err(invalid("authority and legacy versions are metadata, not editable asset fields"));
    }
    Ok(())
}
pub(crate) fn exact(value: &Map<String, Value>, allowed: &[&str]) -> Result<(), Error> {
    if value.keys().any(|key| !allowed.contains(&key.as_str())) {
        Err(invalid("unsupported field in a closed asset shape"))
    } else {
        Ok(())
    }
}

pub(crate) fn patch(before: &[u8], edits: &[FieldEdit]) -> Result<Vec<u8>, Error> {
    let mut value = if before.is_empty() { Map::new() } else { object(before)? };
    let mut paths: Vec<_> = edits
        .iter()
        .map(|edit| match edit {
            FieldEdit::Set { path, .. } | FieldEdit::Delete { path } => path.as_slice(),
        })
        .collect();
    paths.sort_unstable();
    if paths.is_empty()
        || paths.iter().any(|p| p.is_empty() || p.len() > 64)
        || paths.windows(2).any(|p| p[1].starts_with(p[0]))
    {
        return Err(invalid("patch paths must be nonempty and disjoint"));
    }
    for edit in edits {
        let (path, replacement) = match edit {
            FieldEdit::Set { path, value } => (path, Some(value)),
            FieldEdit::Delete { path } => (path, None),
        };
        let mut target = &mut value;
        for component in &path[..path.len() - 1] {
            target = target
                .get_mut(component)
                .and_then(Value::as_object_mut)
                .ok_or_else(|| invalid("patch parent is absent or is not an object; arrays are replaced whole"))?;
        }
        let key = path.last().expect("nonempty path");
        if let Some(replacement) = replacement {
            target.insert(key.clone(), replacement.clone());
        } else {
            target.remove(key);
        }
    }
    bytes(&value)
}

pub(crate) fn differences(before: &Value, after: &Value, max_bytes: usize) -> Result<Vec<FieldChange>, Error> {
    let mut output = Changes { fields: Vec::new(), bytes: 0, max_bytes };
    output.walk(Some(before), Some(after), &mut Vec::new())?;
    Ok(output.fields)
}
struct Changes {
    fields: Vec<FieldChange>,
    bytes: usize,
    max_bytes: usize,
}
impl Changes {
    fn walk(&mut self, before: Option<&Value>, after: Option<&Value>, path: &mut Vec<String>) -> Result<(), Error> {
        if before == after {
            return Ok(());
        }
        if path.len() > 64 {
            return Err(invalid("semantic field path exceeds its depth bound"));
        }
        let left = before.and_then(Value::as_object);
        let right = after.and_then(Value::as_object);
        // Replacing an object with null must still expose every removed child
        // to field authorization, including fields this release cannot write.
        if left.is_some() || right.is_some() {
            let keys: BTreeSet<_> =
                left.into_iter().flat_map(|m| m.keys()).chain(right.into_iter().flat_map(|m| m.keys())).collect();
            if !keys.is_empty() {
                for key in keys {
                    if key.len() > self.max_bytes {
                        return Err(invalid("semantic field name exceeds its budget"));
                    }
                    path.push(key.clone());
                    self.walk(left.and_then(|m| m.get(key)), right.and_then(|m| m.get(key)), path)?;
                    path.pop();
                }
                return Ok(());
            }
        }
        if path.is_empty() {
            return Err(invalid("asset fields require object roots"));
        }
        if self.fields.len() >= 4096 {
            return Err(invalid("too many changed asset fields"));
        }
        let estimate =
            size_of::<FieldChange>() + path.len() * size_of::<String>() + path.iter().map(String::len).sum::<usize>();
        if estimate > self.max_bytes.saturating_sub(self.bytes) {
            return Err(invalid("changed fields exceed their budget"));
        }
        let field = FieldChange { path: path.clone() };
        let cost = field.path.capacity() * size_of::<String>() + field.path.iter().map(String::capacity).sum::<usize>();
        let old = self.fields.capacity();
        self.fields.push(field);
        self.bytes += cost + (self.fields.capacity() - old) * size_of::<FieldChange>();
        if self.bytes > self.max_bytes {
            return Err(invalid("changed fields exceed their budget"));
        }
        Ok(())
    }
}
