//! The public message projection used to confirm a persisted compact replacement.

use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::io;

pub(super) fn order(rows: &[&Value]) -> io::Result<Vec<usize>> {
    let boundary = rows.iter().rposition(|row| row["type"] == "system" && row["subtype"] == "compact_boundary");
    let mut order: Vec<_> = rows
        .iter()
        .enumerate()
        .skip(boundary.unwrap_or(0))
        .filter(|(_, row)| matches!(row["type"].as_str(), Some("user" | "assistant" | "attachment" | "system")))
        .map(|(index, _)| index)
        .collect();
    let Some(boundary) = boundary else { return Ok(order) };
    let by_uuid: HashMap<_, _> =
        rows.iter().enumerate().filter_map(|(index, row)| Some((row["uuid"].as_str()?, index))).collect();
    let metadata = &rows[boundary]["compactMetadata"];
    let (anchor, preserved) = if let Some(messages) = metadata.get("preservedMessages") {
        let anchor = messages["anchorUuid"]
            .as_str()
            .ok_or_else(|| io::Error::other("Claude preserved messages omit their anchor"))?;
        let preserved = messages["uuids"]
            .as_array()
            .ok_or_else(|| io::Error::other("Claude preserved messages omit their UUIDs"))?
            .iter()
            .map(|uuid| uuid.as_str().ok_or_else(|| io::Error::other("Claude preserved UUID is not a string")))
            .collect::<io::Result<Vec<_>>>()?;
        (anchor, preserved)
    } else if let Some(segment) = metadata.get("preservedSegment") {
        let anchor = segment["anchorUuid"]
            .as_str()
            .ok_or_else(|| io::Error::other("Claude preserved segment omits its anchor"))?;
        let head =
            segment["headUuid"].as_str().ok_or_else(|| io::Error::other("Claude preserved segment omits its head"))?;
        let mut tail =
            segment["tailUuid"].as_str().ok_or_else(|| io::Error::other("Claude preserved segment omits its tail"))?;
        let mut visited = HashSet::new();
        let mut preserved = Vec::new();
        loop {
            if !visited.insert(tail) {
                return Err(io::Error::other("Claude preserved segment has a UUID cycle"));
            }
            let index = *by_uuid
                .get(tail)
                .ok_or_else(|| io::Error::other("Claude preserved segment refers to an absent row"))?;
            preserved.push(tail);
            if tail == head {
                break;
            }
            tail = rows[index]["parentUuid"]
                .as_str()
                .ok_or_else(|| io::Error::other("Claude preserved segment does not reach its head"))?;
        }
        preserved.reverse();
        (anchor, preserved)
    } else {
        return Ok(order);
    };
    let mut unique = HashSet::new();
    let preserved: Vec<_> = preserved
        .into_iter()
        .map(|uuid| {
            if !unique.insert(uuid) {
                return Err(io::Error::other("Claude preserved messages repeat a UUID"));
            }
            by_uuid
                .get(uuid)
                .copied()
                .ok_or_else(|| io::Error::other("Claude preserved messages refer to an absent row"))
        })
        .collect::<io::Result<_>>()?;
    order.retain(|index| !preserved.contains(index));
    let anchor = order
        .iter()
        .position(|&index| rows[index]["uuid"].as_str() == Some(anchor))
        .ok_or_else(|| io::Error::other("Claude preserved anchor is absent from the current context"))?;
    order.splice(anchor + 1..anchor + 1, preserved);
    Ok(order)
}

pub(super) fn projected(row: &Value) -> bool {
    row["type"] == "assistant" || row["type"] == "user" && row["isMeta"] != true && row["isVirtual"] != true
}

pub(super) fn projection(row: &Value) -> Value {
    let blocks = row["message"]["content"].as_array().cloned().unwrap_or_else(|| {
        row["message"]["content"].as_str().map(|text| vec![json!({"type":"text", "text":text})]).unwrap_or_default()
    });
    let text: String =
        blocks.iter().filter(|block| block["type"] == "text").filter_map(|block| block["text"].as_str()).collect();
    let uses: Vec<_> = blocks
        .iter()
        .filter(|block| block["type"] == "tool_use")
        .map(|block| {
            json!({
                "tool_use_id":block["id"], "tool":block["name"], "input":block["input"]
            })
        })
        .collect();
    let results: Vec<_> = blocks
        .iter()
        .filter(|block| block["type"] == "tool_result")
        .map(|block| {
            let text = match &block["content"] {
                Value::String(text) => text.clone(),
                Value::Array(parts) => parts
                    .iter()
                    .filter(|part| part["type"] == "text")
                    .map(|part| match &part["text"] {
                        Value::Null => String::new(),
                        Value::String(text) => text.clone(),
                        value => value.to_string(),
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
                _ => String::new(),
            };
            json!({"tool_use_id":block["tool_use_id"], "text":text, "isError":block["is_error"] == true})
        })
        .collect();
    json!({"role":row["type"], "text":text, "toolUses":uses, "toolResults":results})
}

pub(super) fn same(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Number(left), Value::Number(right)) => left.as_f64() == right.as_f64(),
        (Value::Array(left), Value::Array(right)) => {
            left.len() == right.len() && left.iter().zip(right).all(|(left, right)| same(left, right))
        }
        (Value::Object(left), Value::Object(right)) => {
            left.len() == right.len()
                && left.iter().all(|(key, left)| right.get(key).is_some_and(|right| same(left, right)))
        }
        _ => left == right,
    }
}
