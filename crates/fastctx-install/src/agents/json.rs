//! Fastctx's places in an agent's JSON settings, kept as a receipt: a
//! property set, an item added to an array, a whole file of Fastctx's own.
//! Only the bytes of those places change; the rest of the file, its
//! comments, indentation, key order and line endings, stays as the user
//! has it. Taken out, a place still as Fastctx wrote it goes with the very
//! punctuation it came with, or gives back what was there before; one the
//! user changed since stays theirs.

use jsonc_parser::ast::{Array, Object, ObjectProp, Value};
use jsonc_parser::common::Ranged;
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;
use sha2::{Digest, Sha256};

use crate::jsonc::{Document, Insertion};
use crate::{Error, Notice};

/// A property of Fastctx's: the object it is in, its name, what Fastctx
/// wrote, and what was there before.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Placed {
    at: Vec<String>,
    name: String,
    written: Json,
    /// The value that was there before, exactly as written; none where
    /// there was no such property.
    before: Option<String>,
    /// How many of the objects on the way, counted from the deepest,
    /// Fastctx made.
    made: usize,
    /// What Fastctx inserted around the property it added: the property
    /// itself, or the outermost object it made on the way.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    insertion: Option<Insertion>,
    /// There was no file before Fastctx wrote one.
    file_made: bool,
}

impl Placed {
    /// This placing with what was there before taken as Fastctx's own, as
    /// an earlier change that stopped before its receipt was kept left it:
    /// taking it out removes it.
    pub fn owned(self) -> Self {
        Self { before: None, ..self }
    }

    /// The value Fastctx wrote.
    pub fn written(&self) -> &Json {
        &self.written
    }
}

/// An item Fastctx added to an array.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Added {
    at: Vec<String>,
    item: Json,
    /// The item was there before Fastctx came: taking out leaves it.
    found: bool,
    /// How many of the containers on the way, counted from the array
    /// itself, Fastctx made.
    made: usize,
    /// What Fastctx inserted around the outermost container it made.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    insertion: Option<Insertion>,
    file_made: bool,
    /// What Fastctx wrote before the item to set it after the last one, so
    /// that taking it out takes that too.
    gap: String,
}

/// A whole file of Fastctx's own, by the digest of what was written.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OwnFile {
    written: String,
}

/// Puts `value` at `name` in the object at `at`, making the objects on the
/// way. `previous`, Fastctx's earlier placing there, lets a value still as
/// it was written be written anew; one changed since is refused unless
/// `take_back`, and a property Fastctx did not place is refused unless
/// `adopt` (a setting whose value the user may already have chosen) or
/// `take_back`.
#[allow(clippy::too_many_arguments)]
pub fn place(
    bytes: Option<&[u8]>,
    location: &str,
    at: &[&str],
    name: &str,
    value: &Json,
    previous: Option<&Placed>,
    take_back: bool,
    adopt: bool,
) -> Result<(Vec<u8>, Placed), Error> {
    let file_made = previous.map_or(bytes.is_none(), |previous| previous.file_made);
    let text = start(bytes);
    let document = parse(&text, location)?;
    let root = document.root.as_ref().ok_or_else(|| Error::at(location, "the settings must be a JSON object"))?;
    let (object, depth) = walk(root, at, location)?;
    let fresh = |before: Option<String>, made: usize, insertion: Option<Insertion>| Placed {
        at: at.iter().map(|one| (*one).to_owned()).collect(),
        name: name.to_owned(),
        written: value.clone(),
        before,
        made,
        insertion,
        file_made,
    };
    if depth < at.len() {
        // The objects from `at[depth]` on are Fastctx's to make, with the
        // property inside the deepest.
        let keys: Vec<&str> = at[depth + 1..].iter().copied().chain([name]).collect();
        let raw = pretty(&nested(&keys, value.clone()), &document, object);
        let (next, insertion) = document.insert(object, at[depth], &raw).map_err(|error| Error::at(location, error))?;
        return Ok((next, fresh(None, at.len() - depth, Some(insertion))));
    }
    let Some(property) = find(object, name) else {
        let raw = pretty(value, &document, object);
        let (next, insertion) = document.insert(object, name, &raw).map_err(|error| Error::at(location, error))?;
        let placed = match previous {
            Some(previous) => Placed { written: value.clone(), insertion: Some(insertion), ..previous.clone() },
            None => fresh(None, 0, Some(insertion)),
        };
        return Ok((next, placed));
    };
    let current = read(&document, &property.value);
    let ours = previous.is_some_and(|previous| current.as_ref() == Some(&previous.written));
    let placed = match previous {
        Some(previous) if ours || take_back => Placed { written: value.clone(), ..previous.clone() },
        Some(_) => {
            return Err(Error::at(format!("{location}: {}", dotted(at, name)), "changed since Fastctx wrote it"));
        }
        None if current.as_ref() == Some(value) || adopt || take_back => {
            fresh(Some(document.raw(&property.value).to_owned()), 0, None)
        }
        None => {
            return Err(Error::at(
                format!("{location}: {}", dotted(at, name)),
                "is already there and is not Fastctx's",
            ));
        }
    };
    if current.as_ref() == Some(value) {
        return Ok((text.into_bytes(), placed));
    }
    let raw = pretty_at(value, &document, property.range.start);
    let range = property.value.range();
    let next = document.replace(range.start..range.end, &raw).map_err(|error| Error::at(location, error))?;
    Ok((next, placed))
}

/// Takes Fastctx's property out: removed, or what was there before put
/// back, while it is still as Fastctx wrote it; then the objects Fastctx
/// made on the way, where they are left empty, and the file, where Fastctx
/// made it and nothing else is in it. `None` asks for the file's deletion.
pub fn unplace(
    bytes: Option<&[u8]>,
    location: &str,
    placed: &Placed,
) -> Result<(Option<Vec<u8>>, Option<Notice>), Error> {
    let Some(bytes) = bytes else { return Ok((None, None)) };
    let text = String::from_utf8_lossy(bytes).into_owned();
    let document = parse(&text, location)?;
    let Some(root) = document.root.as_ref() else { return Ok((Some(bytes.to_vec()), None)) };
    let at: Vec<&str> = placed.at.iter().map(String::as_str).collect();
    // What Fastctx made on the way, still holding nothing but its property,
    // goes whole with the punctuation it came with.
    if placed.made > 0 {
        let top = at.len() - placed.made.min(at.len());
        let keys: Vec<&str> = at[top + 1..].iter().copied().chain([placed.name.as_str()]).collect();
        if let Some(next) = remove_made(
            &document,
            root,
            &at[..top],
            at[top],
            &nested(&keys, placed.written.clone()),
            placed.insertion.as_ref(),
            location,
        )? {
            return tidy(next, location, &at, 0, placed.file_made).map(|bytes| (bytes, None));
        }
    }
    let (object, depth) = walk(root, &at, location)?;
    if depth < at.len() {
        return Ok((Some(bytes.to_vec()), None));
    }
    let Some(property) = find(object, &placed.name) else {
        return tidy(bytes.to_vec(), location, &at, placed.made, placed.file_made).map(|bytes| (bytes, None));
    };
    if read(&document, &property.value).as_ref() != Some(&placed.written) {
        let notice = Notice {
            location: format!("{location}: {}", dotted(&at, &placed.name)),
            reason: "changed since Fastctx wrote it; left as it is".into(),
        };
        return Ok((Some(bytes.to_vec()), Some(notice)));
    }
    let next = match &placed.before {
        Some(before) => {
            let range = property.value.range();
            document.replace(range.start..range.end, before)
        }
        None => document.remove(object, property, placed.insertion.as_ref().filter(|_| placed.made == 0)),
    }
    .map_err(|error| Error::at(location, error))?;
    tidy(next, location, &at, placed.made, placed.file_made).map(|bytes| (bytes, None))
}

/// Adds `item` to the array at `at`, making it and the objects on the way.
/// An item already there is left as found and is not taken out later.
pub fn add(
    bytes: Option<&[u8]>,
    location: &str,
    at: &[&str],
    item: &Json,
    previous: Option<&Added>,
) -> Result<(Vec<u8>, Added), Error> {
    let file_made = previous.map_or(bytes.is_none(), |previous| previous.file_made);
    let text = start(bytes);
    let document = parse(&text, location)?;
    let root = document.root.as_ref().ok_or_else(|| Error::at(location, "the settings must be a JSON object"))?;
    let (name, parent) = at.split_last().ok_or_else(|| Error::at(location, "an array has no key"))?;
    let (object, depth) = walk(root, parent, location)?;
    let fresh = |found: bool, made: usize, insertion: Option<Insertion>, gap: String| Added {
        at: at.iter().map(|one| (*one).to_owned()).collect(),
        item: item.clone(),
        found,
        made,
        insertion,
        file_made,
        gap,
    };
    let keep = |made: Added| previous.cloned().map_or(made.clone(), |previous| Added { file_made, ..previous });
    if depth < parent.len() {
        // The array, inside the objects from `parent[depth]` on, all made.
        let keys: Vec<&str> = parent[depth + 1..].iter().copied().chain([*name]).collect();
        let raw = pretty(&nested(&keys, Json::Array(vec![item.clone()])), &document, object);
        let (next, insertion) =
            document.insert(object, parent[depth], &raw).map_err(|error| Error::at(location, error))?;
        return Ok((next, keep(fresh(false, at.len() - depth, Some(insertion), String::new()))));
    }
    let Some(property) = find(object, name) else {
        let raw = pretty(&Json::Array(vec![item.clone()]), &document, object);
        let (next, insertion) = document.insert(object, name, &raw).map_err(|error| Error::at(location, error))?;
        return Ok((next, keep(fresh(false, 1, Some(insertion), String::new()))));
    };
    let Value::Array(array) = &property.value else {
        return Err(Error::at(format!("{location}: {}", at.join(".")), "is not an array"));
    };
    if array.elements.iter().any(|element| read(&document, element).as_ref() == Some(item)) {
        return Ok((text.into_bytes(), keep(fresh(true, 0, None, String::new()))));
    }
    let raw = serde_json::to_string(item).expect("a value serializes");
    let (next, gap) = match array.elements.last() {
        None => {
            let range = array.range;
            (document.replace(range.start..range.end, &format!("[{raw}]")), String::new())
        }
        Some(last) => {
            let end = last.range().end;
            let gap = if document.source[array.range.start..array.range.end].contains('\n') {
                format!(",{}{}", document.newline(), document.indent_at(last.range().start))
            } else {
                ", ".to_owned()
            };
            (document.replace(end..end, &format!("{gap}{raw}")), gap)
        }
    };
    let next = next.map_err(|error| Error::at(location, error))?;
    Ok((next, keep(fresh(false, 0, None, gap))))
}

/// Takes Fastctx's item out of the array, where it added it, then the
/// containers it made and left empty.
pub fn unadd(bytes: Option<&[u8]>, location: &str, added: &Added) -> Result<Option<Vec<u8>>, Error> {
    let Some(bytes) = bytes else { return Ok(None) };
    if added.found {
        return Ok(Some(bytes.to_vec()));
    }
    let text = String::from_utf8_lossy(bytes).into_owned();
    let document = parse(&text, location)?;
    let Some(root) = document.root.as_ref() else { return Ok(Some(bytes.to_vec())) };
    let at: Vec<&str> = added.at.iter().map(String::as_str).collect();
    if added.made > 0 {
        let top = at.len() - added.made.min(at.len());
        let expected = nested(&at[top + 1..], Json::Array(vec![added.item.clone()]));
        if let Some(next) =
            remove_made(&document, root, &at[..top], at[top], &expected, added.insertion.as_ref(), location)?
        {
            return tidy(next, location, &at, 0, added.file_made);
        }
    }
    let (name, parent) = at.split_last().ok_or_else(|| Error::at(location, "an array has no key"))?;
    let (object, depth) = walk(root, parent, location)?;
    let array =
        if depth == parent.len() { find(object, name).and_then(|property| as_array(&property.value)) } else { None };
    let Some(array) = array else { return Ok(Some(bytes.to_vec())) };
    let Some(index) = array.elements.iter().position(|element| read(&document, element).as_ref() == Some(&added.item))
    else {
        return Ok(Some(bytes.to_vec()));
    };
    let next = document.remove_element(array, index, &added.gap).map_err(|error| Error::at(location, error))?;
    tidy(next, location, &at, added.made, added.file_made)
}

/// Removes the property `name` of the object at `at`, which Fastctx made,
/// where it still holds exactly `expected`: with the punctuation it came
/// with where that is still around it.
fn remove_made(
    document: &Document<'_>,
    root: &Object<'_>,
    at: &[&str],
    name: &str,
    expected: &Json,
    insertion: Option<&Insertion>,
    location: &str,
) -> Result<Option<Vec<u8>>, Error> {
    let (object, depth) = walk(root, at, location)?;
    if depth < at.len() {
        return Ok(None);
    }
    let Some(property) = find(object, name) else { return Ok(None) };
    if read(document, &property.value).as_ref() != Some(expected) {
        return Ok(None);
    }
    document.remove(object, property, insertion).map(Some).map_err(|error| Error::at(location, error))
}

/// Writes a file of Fastctx's own. One already there as `body`, or as
/// Fastctx last wrote it, is Fastctx's; any other is the user's and is
/// refused unless `take_back`.
pub fn own(
    bytes: Option<&[u8]>,
    location: &str,
    body: &str,
    previous: Option<&OwnFile>,
    take_back: bool,
) -> Result<(Vec<u8>, OwnFile), Error> {
    let written = OwnFile { written: digest(body.as_bytes()) };
    if let Some(bytes) = bytes {
        let same = bytes == body.as_bytes();
        let ours = previous.is_some_and(|previous| previous.written == digest(bytes));
        if !(same || ours || take_back) {
            return Err(Error::at(location, "is already there and is not Fastctx's"));
        }
    }
    Ok((body.as_bytes().to_vec(), written))
}

/// Deletes Fastctx's file, where it is still as Fastctx wrote it.
pub fn unown(bytes: Option<&[u8]>, location: &str, owned: &OwnFile) -> (Option<Vec<u8>>, Option<Notice>) {
    match bytes {
        None => (None, None),
        Some(bytes) if digest(bytes) == owned.written => (None, None),
        Some(bytes) => (
            Some(bytes.to_vec()),
            Some(Notice {
                location: location.to_owned(),
                reason: "changed since Fastctx wrote it; left as it is".into(),
            }),
        ),
    }
}

/// After a place went: each container Fastctx made on the way, from the
/// deepest up, while it is left empty and holds no comment, then the file
/// itself where Fastctx made it and nothing is left in it.
fn tidy(
    mut bytes: Vec<u8>,
    location: &str,
    at: &[&str],
    made: usize,
    file_made: bool,
) -> Result<Option<Vec<u8>>, Error> {
    for level in 0..made.min(at.len()) {
        let path = &at[..at.len() - level];
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let document = parse(&text, location)?;
        let Some(root) = document.root.as_ref() else { break };
        let (name, parent) = path.split_last().expect("a level has a key");
        let (object, depth) = walk(root, parent, location)?;
        if depth < parent.len() {
            break;
        }
        let Some(property) = find(object, name) else { break };
        let empty = match &property.value {
            Value::Object(inner) => inner.properties.is_empty(),
            Value::Array(inner) => inner.elements.is_empty(),
            _ => false,
        };
        if !empty || document.has_comments(property.range.start..property.range.end) {
            break;
        }
        bytes = document.remove(object, property, None).map_err(|error| Error::at(location, error))?;
    }
    if file_made {
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let document = parse(&text, location)?;
        let bare =
            document.root.as_ref().is_none_or(|root| root.properties.is_empty()) && document.comments().is_empty();
        if bare {
            return Ok(None);
        }
    }
    Ok(Some(bytes))
}

/// The text a settings file is worked on as: an empty object where there
/// is none, laid out over lines so what is added is too.
fn start(bytes: Option<&[u8]>) -> String {
    match bytes {
        Some(bytes) if !String::from_utf8_lossy(bytes).trim().is_empty() => String::from_utf8_lossy(bytes).into_owned(),
        _ => "{\n}\n".to_owned(),
    }
}

fn parse<'a>(text: &'a str, location: &str) -> Result<Document<'a>, Error> {
    Document::parse(text.as_bytes()).map_err(|error| Error::at(location, error))
}

/// The object at `at`, as deep as it goes: the deepest object found and how
/// many keys of `at` it took to reach it.
fn walk<'a, 'b>(root: &'b Object<'a>, at: &[&str], location: &str) -> Result<(&'b Object<'a>, usize), Error> {
    let mut object = root;
    for (depth, key) in at.iter().enumerate() {
        match find(object, key) {
            None => return Ok((object, depth)),
            Some(property) => match &property.value {
                Value::Object(inner) => object = inner,
                _ => return Err(Error::at(format!("{location}: {}", at[..=depth].join(".")), "is not an object")),
            },
        }
    }
    Ok((object, at.len()))
}

fn find<'a, 'b>(object: &'b Object<'a>, name: &str) -> Option<&'b ObjectProp<'a>> {
    object.properties.iter().find(|property| property.name.as_str() == name)
}

fn as_array<'a, 'b>(value: &'b Value<'a>) -> Option<&'b Array<'a>> {
    match value {
        Value::Array(array) => Some(array),
        _ => None,
    }
}

/// `leaf` inside an object for each of `keys`, the outermost first.
fn nested(keys: &[&str], leaf: Json) -> Json {
    keys.iter().rev().fold(leaf, |inner, key| {
        let mut outer = serde_json::Map::new();
        outer.insert((*key).to_owned(), inner);
        Json::Object(outer)
    })
}

/// A value as JSON, where it is plain JSON; one holding a comment is the
/// user's own writing, never Fastctx's.
fn read(document: &Document<'_>, value: &Value<'_>) -> Option<Json> {
    serde_json::from_str(document.raw(value)).ok()
}

/// `value` laid out to stand as a new property of `object`.
fn pretty(value: &Json, document: &Document<'_>, object: &Object<'_>) -> String {
    match document.inner_indent(object) {
        Some(indent) => lay_out(value, &indent, document.newline()),
        None => serde_json::to_string(value).expect("a value serializes"),
    }
}

/// `value` laid out to stand where a property starting at `position` is.
fn pretty_at(value: &Json, document: &Document<'_>, position: usize) -> String {
    lay_out(value, &document.indent_at(position), document.newline())
}

/// `value` over lines, each after the first indented from `indent`.
fn lay_out(value: &Json, indent: &str, newline: &str) -> String {
    let text = serde_json::to_string_pretty(value).expect("a value serializes");
    text.split('\n').collect::<Vec<_>>().join(&format!("{newline}{indent}"))
}

fn dotted(at: &[&str], name: &str) -> String {
    at.iter().copied().chain([name]).collect::<Vec<_>>().join(".")
}

fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|byte| format!("{byte:02x}")).collect()
}
