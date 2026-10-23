//! Long strings retain exact UTF-8 bytes in bounded, independently readable account chunks.

use super::{Scope, TEXT, Writer, failure, immutable, key, read};
use crate::Node;
use fastctx_types::access::HubAccount;
use fastctx_types::ipc::Failure;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

const LONG_TEXT: usize = 4096;
// Even a string made entirely of JSON control escapes fits beneath the account's 64 KiB record limit.
const CHUNK_BYTES: usize = 8192;

#[derive(Clone, Default, Serialize, Deserialize)]
pub(crate) struct Storage {
    #[serde(skip)]
    pub(super) owner: Option<String>,
    version: u8,
    pub(super) texts: BTreeMap<String, TextRef>,
    fields: Vec<Field>,
}

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct TextRef {
    keys: Vec<String>,
    bytes: usize,
}

#[derive(Clone, Serialize, Deserialize)]
struct Field {
    path: String,
    digest: String,
}

#[derive(Serialize, Deserialize)]
struct Chunk {
    #[serde(flatten)]
    scope: Scope,
    text: String,
}

fn strings(value: &mut Value, path: String, found: &mut Vec<(String, String)>) {
    match value {
        Value::String(text) if text.len() > LONG_TEXT => found.push((path, std::mem::take(text))),
        Value::Array(array) => {
            for (index, value) in array.iter_mut().enumerate() {
                strings(value, format!("{path}/{index}"), found);
            }
        }
        Value::Object(object) => {
            for (name, value) in object.iter_mut() {
                strings(value, format!("{path}/{}", name.replace('~', "~0").replace('/', "~1")), found);
            }
        }
        _ => {}
    }
}

pub(super) async fn encode(writer: &mut Writer<'_>, mut value: Value) -> Result<Value, Failure> {
    let mut long = Vec::new();
    strings(&mut value, String::new(), &mut long);
    if long.is_empty() {
        return Ok(value);
    }
    let mut metadata = Storage { version: 1, ..Default::default() };
    for (path, text) in long {
        let digest = fastctx_hands::files::content_hash(text.as_bytes());
        let reference = if let Some(reference) = writer.texts.get(&digest) {
            reference.clone()
        } else {
            writer.reserve().await?;
            let mut keys = Vec::new();
            let mut rest = text.as_str();
            while !rest.is_empty() {
                let mut end = CHUNK_BYTES.min(rest.len());
                while !rest.is_char_boundary(end) {
                    end -= 1;
                }
                let (part, remaining) = rest.split_at(end);
                let body =
                    serde_json::to_value(Chunk { scope: writer.scope.clone(), text: part.into() }).map_err(failure)?;
                let key = key("t", &serde_json::to_vec(&body).map_err(failure)?);
                immutable(writer.node, writer.account, TEXT, &key, body).await?;
                keys.push(key);
                rest = remaining;
            }
            let reference = TextRef { keys, bytes: text.len() };
            writer.texts.insert(digest.clone(), reference.clone());
            reference
        };
        metadata.texts.insert(digest.clone(), reference);
        metadata.fields.push(Field { path, digest });
    }
    value["_goal_storage"] = serde_json::to_value(metadata).map_err(failure)?;
    Ok(value)
}

pub(crate) async fn decode(
    node: &Arc<Node>,
    account: &HubAccount,
    mut value: Value,
) -> Result<(Value, Storage), Failure> {
    let owner = value["id"].as_str().or_else(|| value["goal"].as_str()).map(str::to_string);
    let Some(metadata) = value.as_object_mut().and_then(|object| object.remove("_goal_storage")) else {
        return Ok((value, Storage { owner, ..Default::default() }));
    };
    let mut metadata: Storage = serde_json::from_value(metadata).map_err(failure)?;
    metadata.owner = owner;
    if metadata.version != 1 {
        return Err(Failure::new("this goal storage format requires a newer node"));
    }
    let goal = value["id"]
        .as_str()
        .or_else(|| value["goal"].as_str())
        .ok_or_else(|| Failure::new("goal text has no owner"))?
        .to_string();
    let resident = value["resident"]
        .as_str()
        .or_else(|| value["resident"]["resident"].as_str())
        .ok_or_else(|| Failure::new("goal text has no conversation"))?
        .to_string();
    let mut texts = BTreeMap::new();
    for (digest, reference) in &metadata.texts {
        let mut tasks = tokio::task::JoinSet::new();
        let mut input = reference.keys.iter().enumerate();
        let mut pieces = vec![String::new(); reference.keys.len()];
        loop {
            while tasks.len() < 16 {
                let Some((index, key)) = input.next() else {
                    break;
                };
                let (node, account, key, goal, resident) =
                    (node.clone(), account.clone(), key.clone(), goal.clone(), resident.clone());
                tasks.spawn(async move {
                    let body = read(&node, &account, TEXT, &key)
                        .await?
                        .and_then(|record| record.body)
                        .ok_or_else(|| Failure::new(format!("goal text chunk {key} is missing")))?;
                    if super::key("t", &serde_json::to_vec(&body).map_err(failure)?) != key {
                        return Err(Failure::new("goal text chunk content does not match its reference"));
                    }
                    let chunk: Chunk = serde_json::from_value(body).map_err(failure)?;
                    if chunk.scope.goal != goal || chunk.scope.resident.as_str() != resident {
                        return Err(Failure::new("goal text chunk belongs to another conversation"));
                    }
                    Ok::<_, Failure>((index, chunk.text))
                });
            }
            let Some(done) = tasks.join_next().await else {
                break;
            };
            let (index, text) = done.map_err(failure)??;
            pieces[index] = text;
        }
        let text = pieces.concat();
        if text.len() != reference.bytes || fastctx_hands::files::content_hash(text.as_bytes()) != *digest {
            return Err(Failure::new("goal text bytes do not match their accepted reference"));
        }
        texts.insert(digest, text);
    }
    let mut paths = BTreeSet::new();
    for field in &metadata.fields {
        if !paths.insert(&field.path) {
            return Err(Failure::new("goal text has a duplicate field reference"));
        }
        let target = value.pointer_mut(&field.path).ok_or_else(|| Failure::new("goal text field is missing"))?;
        if target.as_str() != Some("") {
            return Err(Failure::new("goal text field is not a storage placeholder"));
        }
        *target = Value::String(
            texts.get(&field.digest).ok_or_else(|| Failure::new("goal text reference is missing"))?.clone(),
        );
    }
    Ok((value, metadata))
}
