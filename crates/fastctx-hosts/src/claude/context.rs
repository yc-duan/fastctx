//! The public message projection used to confirm a persisted compact replacement.

use serde_json::{Value, json};
use std::collections::HashSet;
use std::io;

/// The rows of the context the session holds, in their order: from the last
/// compaction that replaced it (`boundary`), the rows of the kinds a context
/// is made of (`contextual`, all of them in order), the messages it kept put
/// back after its anchor. Read through the row source (`Rows`): by number,
/// and by the UUID a row bears, each UUID one row's. What the rows say that
/// makes no context is told apart (the inner `Err`), as a problem of the
/// record; a row the source fails to give fails the reading. Rows are
/// numbered in four bytes (the reading counts no more).
pub(super) fn order(
    rows: &dyn super::transcript::Rows,
    contextual: Vec<u32>,
    boundary: Option<usize>,
) -> io::Result<Result<Vec<u32>, &'static str>> {
    match ordered(rows, contextual, boundary) {
        Ok(order) => Ok(Ok(order)),
        Err(Untold::Context(why)) => Ok(Err(why)),
        Err(Untold::Store(error)) => Err(error),
    }
}

/// Why `order` could not tell the context: the row source failed, or the
/// rows make no context.
enum Untold {
    Store(io::Error),
    Context(&'static str),
}

impl From<io::Error> for Untold {
    fn from(error: io::Error) -> Self {
        Untold::Store(error)
    }
}

fn ordered(rows: &dyn super::transcript::Rows, contextual: Vec<u32>, boundary: Option<usize>) -> Result<Vec<u32>, Untold> {
    use Untold::Context;
    let mut order: Vec<u32> = contextual.into_iter().filter(|index| *index as usize >= boundary.unwrap_or(0)).collect();
    let Some(boundary) = boundary else { return Ok(order) };
    let boundary_row = rows.row(boundary)?;
    let metadata = &boundary_row.value_of()["compactMetadata"];
    // The rows the compaction kept, by their numbers — a UUID each row's
    // and each row's one — not by their UUIDs held again: a segment may be
    // the whole of a long record.
    let (anchor, preserved, kept) = if let Some(messages) = metadata.get("preservedMessages") {
        let anchor = messages["anchorUuid"].as_str().ok_or(Context("Claude preserved messages omit their anchor"))?;
        let uuids = messages["uuids"].as_array().ok_or(Context("Claude preserved messages omit their UUIDs"))?;
        for uuid in uuids {
            uuid.as_str().ok_or(Context("Claude preserved UUID is not a string"))?;
        }
        // Each told once, of a row there is: the first fault met says why.
        let (mut kept, mut preserved) = (HashSet::new(), Vec::with_capacity(uuids.len()));
        for uuid in uuids.iter().filter_map(Value::as_str) {
            let index = rows.index_of(uuid)?.ok_or(Context("Claude preserved messages refer to an absent row"))? as u32;
            if !kept.insert(index) {
                return Err(Context("Claude preserved messages repeat a UUID"));
            }
            preserved.push(index);
        }
        (anchor, preserved, kept)
    } else if let Some(segment) = metadata.get("preservedSegment") {
        let anchor = segment["anchorUuid"].as_str().ok_or(Context("Claude preserved segment omits its anchor"))?;
        let head = segment["headUuid"].as_str().ok_or(Context("Claude preserved segment omits its head"))?;
        let mut tail = segment["tailUuid"].as_str().ok_or(Context("Claude preserved segment omits its tail"))?.to_owned();
        let (mut kept, mut preserved) = (HashSet::new(), Vec::new());
        loop {
            let index = rows.index_of(&tail)?.ok_or(Context("Claude preserved segment refers to an absent row"))?;
            // A row met again is a UUID met again: a cycle.
            if !kept.insert(index as u32) {
                return Err(Context("Claude preserved segment has a UUID cycle"));
            }
            preserved.push(index as u32);
            if tail == head {
                break;
            }
            tail = rows.row(index)?.value_of()["parentUuid"]
                .as_str()
                .ok_or(Context("Claude preserved segment does not reach its head"))?
                .to_owned();
        }
        preserved.reverse();
        (anchor, preserved, kept)
    } else {
        return Ok(order);
    };
    order.retain(|index| !kept.contains(index));
    // The anchor's row: the one row bearing its UUID, where it stands in
    // the order (a row numbered once bears one UUID, each UUID one row's).
    let anchor_row = rows.index_of(anchor)?;
    let anchor = anchor_row
        .and_then(|anchor_row| order.iter().position(|&index| index as usize == anchor_row))
        .ok_or(Context("Claude preserved anchor is absent from the current context"))?;
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
