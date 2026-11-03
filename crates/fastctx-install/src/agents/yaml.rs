//! Fastctx's places in an agent's YAML settings, kept as a receipt: an
//! entry of a mapping, an item of a list, a value set. YAML is changed as
//! text, whole lines added or taken out where the outline of the file puts
//! them, so the rest of the file, its comments, quoting and layout, stays as
//! the user has it. Every change is read back whole and must mean exactly
//! the settings intended, or it is not made: a file laid out in a way the
//! outline does not follow is refused, never rewritten.

use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

use crate::{Error, Notice};

/// A key whose empty value, written on its line, gave way to Fastctx's
/// lines, and that text.
type Emptied = (Vec<String>, String);

/// An entry of Fastctx's in a mapping, or a setting it set.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Placed {
    at: Vec<String>,
    name: String,
    written: Json,
    /// The setting's text as it was before Fastctx set it; none where there
    /// was no such key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    before: Option<String>,
    /// How many of the mappings on the way, counted from the deepest,
    /// Fastctx made.
    made: usize,
    /// The key whose empty value, written on its line as `{}`, `null` or
    /// `~`, gave way to Fastctx's lines, and that text, put back once its
    /// mapping or list is empty again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    emptied: Option<Emptied>,
    file_made: bool,
}

impl Placed {
    /// This placing in a file that was not there before the change it came
    /// with, which another of its places may have made first.
    pub fn in_made_file(self) -> Self {
        Self { file_made: true, ..self }
    }

    pub fn written(&self) -> &Json {
        &self.written
    }
}

/// An item Fastctx added to a list.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Added {
    at: Vec<String>,
    item: Json,
    /// The item was there before Fastctx came: taking out leaves it.
    found: bool,
    /// How many of the containers on the way, counted from the list itself,
    /// Fastctx made.
    made: usize,
    /// The key whose empty value, written on its line as `{}`, `null` or
    /// `~`, gave way to Fastctx's lines, and that text, put back once its
    /// mapping or list is empty again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    emptied: Option<Emptied>,
    file_made: bool,
}

impl Added {
    /// This item in a file that was not there before the change it came
    /// with, which another of its places may have made first.
    pub fn in_made_file(self) -> Self {
        Self { file_made: true, ..self }
    }
}

/// The settings `bytes` hold, as JSON; none where there is no file, and
/// null where it holds nothing.
pub fn read(bytes: Option<&[u8]>, location: &str) -> Result<Option<Json>, Error> {
    let Some(bytes) = bytes else { return Ok(None) };
    let text = std::str::from_utf8(bytes).map_err(|_| Error::at(location, "is not UTF-8 text"))?;
    parse(text, location).map(Some)
}

/// The value at `at` in the settings `bytes` hold.
pub fn value_at(bytes: Option<&[u8]>, location: &str, at: &[&str]) -> Result<Option<Json>, Error> {
    Ok(read(bytes, location)?.and_then(|settings| at_path(&settings, at).cloned()))
}

/// Puts `value` at `name` in the mapping at `at`, making the mappings on the
/// way. `previous`, Fastctx's earlier placing there, lets a value still as
/// it was written be written anew; one changed since is refused unless
/// `take_back`, and a key Fastctx did not place is refused unless `adopt` (a
/// setting the user may already have chosen) or `take_back`.
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
    let text = text_of(bytes, location)?;
    let settings = parse(&text, location)?;
    let place = format!("{location}: {}", dotted(at, name));
    let (depth, current) = reach(&settings, at, name, &place)?;
    let fresh = |before: Option<String>, made: usize, emptied: Option<Emptied>| Placed {
        at: owned(at),
        name: name.to_owned(),
        written: value.clone(),
        before,
        made,
        emptied,
        file_made,
    };
    let Some(current) = current else {
        let (next, emptied) = Outline::of(&text).insert_entry(&at[..depth], &at[depth..], name, value, &place)?;
        let next = checked(next, &settings, &place, |expected| set_path(expected, at, name, value.clone()))?;
        let placed = match previous {
            Some(previous) => Placed { written: value.clone(), ..previous.clone() },
            None => fresh(None, at.len() - depth, emptied.map(|text| (owned(&at[..depth]), text))),
        };
        return Ok((next, placed));
    };
    let ours = previous.is_some_and(|previous| current == previous.written);
    let placed = match previous {
        Some(previous) if ours || take_back => Placed { written: value.clone(), ..previous.clone() },
        Some(_) => return Err(Error::at(place, "changed since Fastctx wrote it")),
        None if current == *value || adopt || take_back => {
            fresh(Some(Outline::of(&text).scalar_text(at, name, &place)?), 0, None)
        }
        None => return Err(Error::at(place, "is already there and is not Fastctx's")),
    };
    if current == *value {
        return Ok((text.into_bytes(), placed));
    }
    let next = Outline::of(&text).replace_scalar(at, name, &scalar(value, &place)?, &place)?;
    let next = checked(next, &settings, &place, |expected| set_path(expected, at, name, value.clone()))?;
    Ok((next, placed))
}

/// Takes Fastctx's entry out: removed, or what was there before put back,
/// while it is still as Fastctx wrote it; then the mappings Fastctx made on
/// the way, where they are left empty, and the file, where Fastctx made it
/// and nothing else is in it. `None` asks for the file's deletion.
pub fn unplace(
    bytes: Option<&[u8]>,
    location: &str,
    placed: &Placed,
) -> Result<(Option<Vec<u8>>, Option<Notice>), Error> {
    let Some(bytes) = bytes else { return Ok((None, None)) };
    let text = text_of(Some(bytes), location)?;
    let settings = parse(&text, location)?;
    let at: Vec<&str> = placed.at.iter().map(String::as_str).collect();
    let place = format!("{location}: {}", dotted(&at, &placed.name));
    let Some(current) = at_path(&settings, &at).and_then(|object| object.get(&placed.name)) else {
        return Ok((Some(bytes.to_vec()), None));
    };
    if *current != placed.written {
        let notice = Notice { location: place, reason: "changed since Fastctx wrote it; left as it is".into() };
        return Ok((Some(bytes.to_vec()), Some(notice)));
    }
    let next = match &placed.before {
        Some(before) => {
            let next = Outline::of(&text).replace_scalar(&at, &placed.name, before, &place)?;
            let restored = parse(&next, location)?;
            check_same_but(&settings, &restored, &at, &placed.name, &place)?;
            next
        }
        None => {
            let next = Outline::of(&text).remove_entry(&at, &placed.name, placed.made, &place)?;
            let next = checked_text(next, &settings, &place, |expected| {
                remove_path(expected, &at, &placed.name);
                prune(expected, &at, placed.made);
            })?;
            refilled(next, placed.emptied.as_ref(), &place)?
        }
    };
    Ok((finish(next, placed.file_made), None))
}

/// Whether the entry `placed` names is still there, whatever it holds.
pub fn holds_placed(bytes: Option<&[u8]>, location: &str, placed: &Placed) -> Result<bool, Error> {
    let at: Vec<&str> = placed.at.iter().map(String::as_str).collect();
    Ok(read(bytes, location)?
        .is_some_and(|settings| at_path(&settings, &at).and_then(|object| object.get(&placed.name)).is_some()))
}

/// Adds `item` to the list at `at`, making it and the mappings on the way.
/// An item already there is left as found and is not taken out later.
/// `ours` tells Fastctx's own item from the others, where one Fastctx
/// wrote before is to be written anew (a server's entry): it is replaced
/// while still as `previous` wrote it, or with `take_back`.
pub fn add(
    bytes: Option<&[u8]>,
    location: &str,
    at: &[&str],
    item: &Json,
    previous: Option<&Added>,
    ours: Option<&dyn Fn(&Json) -> bool>,
    take_back: bool,
) -> Result<(Vec<u8>, Added), Error> {
    let file_made = previous.map_or(bytes.is_none(), |previous| previous.file_made);
    let text = text_of(bytes, location)?;
    let settings = parse(&text, location)?;
    let place = format!("{location}: {}", if at.is_empty() { "the list".to_owned() } else { at.join(".") });
    let (there, old) = {
        let items: &[Json] = match at_path(&settings, at) {
            None | Some(Json::Null) => &[],
            Some(Json::Array(items)) => items,
            Some(_) => return Err(Error::at(place, "is not a list")),
        };
        (items.iter().any(|one| one == item), ours.and_then(|ours| items.iter().find(|one| ours(one)).cloned()))
    };
    let keep = |made: Added| {
        previous.cloned().map_or(made.clone(), |previous| Added { file_made, item: item.clone(), ..previous })
    };
    if there {
        let found = previous.is_none_or(|previous| previous.found);
        let added = Added { at: owned(at), item: item.clone(), found, made: 0, emptied: None, file_made };
        return Ok((text.into_bytes(), keep(added)));
    }
    // Fastctx's own, written before: written anew where still as written.
    let mut text = text;
    let mut settings = settings;
    if let Some(old) = old {
        let written = previous.is_some_and(|previous| previous.item == old);
        if !(written || take_back) {
            return Err(Error::at(
                place,
                if previous.is_some() {
                    "changed since Fastctx wrote it"
                } else {
                    "is already there and is not Fastctx's"
                },
            ));
        }
        let next = Outline::of(&text).remove_item(at, &old, &place)?;
        let next = checked_text(next, &settings, &place, |expected| remove_item_path(expected, at, &old))?;
        settings = parse(&next, location)?;
        text = next;
    }
    let made = previous.map_or_else(|| missing(&settings, at), |previous| previous.made);
    let (next, emptied) = Outline::of(&text).append_item(at, item, &settings, &place)?;
    let next = checked(next, &settings, &place, |expected| push_path(expected, at, item.clone()))?;
    let emptied = previous.map_or(emptied, |previous| previous.emptied.clone());
    Ok((next, keep(Added { at: owned(at), item: item.clone(), found: false, made, emptied, file_made })))
}

/// Takes Fastctx's item out of the list, where it added it, then the
/// containers it made and left empty.
pub fn unadd(bytes: Option<&[u8]>, location: &str, added: &Added) -> Result<(Option<Vec<u8>>, Option<Notice>), Error> {
    let Some(bytes) = bytes else { return Ok((None, None)) };
    if added.found {
        return Ok((Some(bytes.to_vec()), None));
    }
    let text = text_of(Some(bytes), location)?;
    let settings = parse(&text, location)?;
    let at: Vec<&str> = added.at.iter().map(String::as_str).collect();
    let place = format!("{location}: {}", if at.is_empty() { "the list".to_owned() } else { at.join(".") });
    let holds = matches!(at_path(&settings, &at), Some(Json::Array(items)) if items.contains(&added.item));
    if !holds {
        return Ok((Some(bytes.to_vec()), None));
    }
    let next = Outline::of(&text).remove_item(&at, &added.item, &place)?;
    let next = checked_text(next, &settings, &place, |expected| remove_item_path(expected, &at, &added.item))?;
    let now = parse(&next, location)?;
    // The list and the mappings Fastctx made for it go where left empty.
    let mut next = next;
    if added.made > 0 && !at.is_empty() && at_path(&now, &at).is_none_or(|list| same(list, &Json::Null)) {
        let (name, parent) = at.split_last().expect("not empty");
        let removed = Outline::of(&next).remove_entry(parent, name, added.made - 1, &place)?;
        next = checked_text(removed, &now, &place, |expected| {
            remove_path(expected, parent, name);
            prune(expected, parent, added.made - 1);
        })?;
    }
    let next = refilled(next, added.emptied.as_ref(), &place)?;
    Ok((finish(next, added.file_made), None))
}

/// `text` with the empty value Fastctx wrote into put back on its key's
/// line, where that mapping or list is empty again.
fn refilled(text: String, emptied: Option<&Emptied>, place: &str) -> Result<String, Error> {
    let Some((at, empty)) = emptied else { return Ok(text) };
    let at: Vec<&str> = at.iter().map(String::as_str).collect();
    let outline = Outline::of(&text);
    let Some(region) = outline.region(&at) else { return Ok(text) };
    let Some(key) = region.key else { return Ok(text) };
    let Kind::Key { inline, colon_at, .. } = &outline.lines[key].kind else { return Ok(text) };
    if !inline.is_empty() || outline.first_content(region.first, region.end).is_some() {
        return Ok(text);
    }
    let next = format!("{}{empty}{}", &text[..*colon_at], &text[*colon_at..]);
    if !same(&parse(&next, place)?, &parse(&text, place)?) {
        return Ok(text);
    }
    Ok(next)
}

/// Whether the item `added` is still in its list: one that was there
/// before Fastctx came is not Fastctx's to miss.
pub fn holds_added(bytes: Option<&[u8]>, location: &str, added: &Added) -> Result<bool, Error> {
    if added.found {
        return Ok(true);
    }
    let at: Vec<&str> = added.at.iter().map(String::as_str).collect();
    Ok(read(bytes, location)?.is_some_and(
        |settings| matches!(at_path(&settings, &at), Some(Json::Array(items)) if items.contains(&added.item)),
    ))
}

fn text_of(bytes: Option<&[u8]>, location: &str) -> Result<String, Error> {
    match bytes {
        None => Ok(String::new()),
        Some(bytes) => {
            std::str::from_utf8(bytes).map(str::to_owned).map_err(|_| Error::at(location, "is not UTF-8 text"))
        }
    }
}

fn parse(text: &str, location: &str) -> Result<Json, Error> {
    if text.trim().is_empty() {
        return Ok(Json::Null);
    }
    serde_yaml_ng::from_str::<Json>(text)
        .map_err(|error| Error::at(location, format!("is not YAML Fastctx can read: {error}")))
}

/// `next` where it means `settings` changed as `change` does; refused
/// otherwise, as the file is laid out in a way the outline did not follow.
fn checked(next: String, settings: &Json, place: &str, change: impl FnOnce(&mut Json)) -> Result<Vec<u8>, Error> {
    checked_text(next, settings, place, change).map(String::into_bytes)
}

fn checked_text(next: String, settings: &Json, place: &str, change: impl FnOnce(&mut Json)) -> Result<String, Error> {
    let mut expected = settings.clone();
    change(&mut expected);
    if !same(&parse(&next, place)?, &expected) {
        return Err(Error::at(place, "is laid out in a way Fastctx cannot change without rewriting the file"));
    }
    Ok(next)
}

/// Whether two settings mean the same, an empty mapping or list being no
/// value: the lines of a key whose last entry went read as nothing.
fn same(one: &Json, other: &Json) -> bool {
    fn bare(value: &Json) -> Json {
        match value {
            Json::Object(object) if object.is_empty() => Json::Null,
            Json::Array(items) if items.is_empty() => Json::Null,
            Json::Object(object) => {
                Json::Object(object.iter().map(|(key, value)| (key.clone(), bare(value))).collect())
            }
            Json::Array(items) => Json::Array(items.iter().map(bare).collect()),
            value => value.clone(),
        }
    }
    bare(one) == bare(other)
}

/// That `after` differs from `before` only in the setting at `at`.`name`.
fn check_same_but(before: &Json, after: &Json, at: &[&str], name: &str, place: &str) -> Result<(), Error> {
    let (mut before, mut after) = (before.clone(), after.clone());
    remove_path(&mut before, at, name);
    remove_path(&mut after, at, name);
    if before != after {
        return Err(Error::at(place, "is laid out in a way Fastctx cannot change without rewriting the file"));
    }
    Ok(())
}

/// The file's text after a place went, `None` where Fastctx made the file
/// and nothing but comments-free blank text is left.
fn finish(text: String, file_made: bool) -> Option<Vec<u8>> {
    if file_made && text.trim().is_empty() { None } else { Some(text.into_bytes()) }
}

/// How many keys of `at` are there, a key with no value counting as an
/// empty mapping, and what `name` holds at its end where it is all there.
fn reach(settings: &Json, at: &[&str], name: &str, place: &str) -> Result<(usize, Option<Json>), Error> {
    let mut value = settings;
    for (depth, key) in at.iter().enumerate() {
        match value {
            Json::Object(object) => match object.get(*key) {
                None => return Ok((depth, None)),
                Some(Json::Null) => return Ok((depth + 1, None)),
                Some(inner) => value = inner,
            },
            Json::Null if depth == 0 => return Ok((0, None)),
            _ => return Err(Error::at(place, format!("{} is not a mapping", at[..depth].join(".")))),
        }
    }
    match value {
        Json::Object(object) => Ok((at.len(), object.get(name).cloned())),
        Json::Null => Ok((at.len(), None)),
        _ => Err(Error::at(place, "is not inside a mapping")),
    }
}

/// How many of the containers of `at`, counted from the deepest, are not
/// there yet: the list itself and the mappings on its way.
fn missing(settings: &Json, at: &[&str]) -> usize {
    let mut value = settings;
    for (depth, key) in at.iter().enumerate() {
        match value.get(*key) {
            None => return at.len() - depth,
            // There, with nothing in it: the user's, and the rest is made in it.
            Some(Json::Null) => return at.len() - depth - 1,
            Some(inner) => value = inner,
        }
    }
    0
}

fn at_path<'a>(settings: &'a Json, at: &[&str]) -> Option<&'a Json> {
    at.iter().try_fold(settings, |value, key| value.get(*key))
}

fn set_path(settings: &mut Json, at: &[&str], name: &str, value: Json) {
    let mut place = settings;
    for key in at {
        if !place.is_object() {
            *place = Json::Object(serde_json::Map::new());
        }
        let object = place.as_object_mut().expect("an object");
        let next = object.entry((*key).to_owned()).or_insert(Json::Null);
        place = next;
    }
    if !place.is_object() {
        *place = Json::Object(serde_json::Map::new());
    }
    place.as_object_mut().expect("an object").insert(name.to_owned(), value);
}

fn remove_path(settings: &mut Json, at: &[&str], name: &str) {
    let mut place = settings;
    for key in at {
        match place.get_mut(*key) {
            Some(inner) => place = inner,
            None => return,
        }
    }
    if let Some(object) = place.as_object_mut() {
        object.remove(name);
    }
}

/// The mappings Fastctx made on the way to `at`, from the deepest up, while
/// left empty.
fn prune(settings: &mut Json, at: &[&str], made: usize) {
    for level in 0..made.min(at.len()) {
        let path = &at[..at.len() - level];
        let (name, parent) = path.split_last().expect("a level has a key");
        let empty = at_path(settings, path).is_some_and(|value| match value {
            Json::Object(object) => object.is_empty(),
            Json::Null => true,
            _ => false,
        });
        if !empty {
            break;
        }
        remove_path(settings, parent, name);
    }
}

fn push_path(settings: &mut Json, at: &[&str], item: Json) {
    if at.is_empty() {
        if !settings.is_array() {
            *settings = Json::Array(Vec::new());
        }
        settings.as_array_mut().expect("an array").push(item);
        return;
    }
    let (name, parent) = at.split_last().expect("not empty");
    let mut place = &mut *settings;
    for key in parent {
        if !place.is_object() {
            *place = Json::Object(serde_json::Map::new());
        }
        place = place.as_object_mut().expect("an object").entry((*key).to_owned()).or_insert(Json::Null);
    }
    if !place.is_object() {
        *place = Json::Object(serde_json::Map::new());
    }
    let list = place.as_object_mut().expect("an object").entry((*name).to_owned()).or_insert(Json::Null);
    if !list.is_array() {
        *list = Json::Array(Vec::new());
    }
    list.as_array_mut().expect("an array").push(item);
}

fn remove_item_path(settings: &mut Json, at: &[&str], item: &Json) {
    let mut place = settings;
    for key in at {
        match place.get_mut(*key) {
            Some(inner) => place = inner,
            None => return,
        }
    }
    if let Some(items) = place.as_array_mut()
        && let Some(index) = items.iter().position(|one| one == item)
    {
        items.remove(index);
    }
}

fn owned(at: &[&str]) -> Vec<String> {
    at.iter().map(|one| (*one).to_owned()).collect()
}

fn dotted(at: &[&str], name: &str) -> String {
    at.iter().copied().chain([name]).collect::<Vec<_>>().join(".")
}

/// A scalar as YAML writes it on one line.
fn scalar(value: &Json, place: &str) -> Result<String, Error> {
    if value.is_object() || value.is_array() {
        return Err(Error::at(place, "only a single value is set in YAML"));
    }
    let text = serde_yaml_ng::to_string(value).map_err(|error| Error::at(place, error.to_string()))?;
    Ok(text.trim_end().to_owned())
}

/// `value` as block YAML, each line after `indent`, ending with a newline.
fn block(value: &Json, indent: usize, newline: &str) -> Result<String, Error> {
    let text = serde_yaml_ng::to_string(value).map_err(|error| Error::at("YAML", error.to_string()))?;
    let pad = " ".repeat(indent);
    Ok(text
        .lines()
        .map(|line| if line.is_empty() { newline.to_owned() } else { format!("{pad}{line}{newline}") })
        .collect())
}

/// One line of a YAML file as its outline sees it.
#[derive(Clone, Debug)]
struct Line {
    start: usize,
    /// Past its line break, if any.
    end: usize,
    /// Its content's end, before any line break.
    stop: usize,
    indent: usize,
    kind: Kind,
}

#[derive(Clone, Debug, PartialEq)]
enum Kind {
    Blank,
    Comment,
    /// `name:` and what follows it on the line, a comment left out; where
    /// the colon ends and the value starts, in the file.
    Key {
        name: String,
        inline: String,
        colon_at: usize,
        value_at: usize,
    },
    /// `- ` and what follows.
    Item,
    Other,
}

impl Line {
    fn content(&self) -> bool {
        !matches!(self.kind, Kind::Blank | Kind::Comment)
    }
}

/// The lines of a YAML file and the mappings and lists they make by their
/// indentation: block YAML as configuration files are written. Flow
/// collections are only met as a whole value on one line.
struct Outline<'a> {
    text: &'a str,
    lines: Vec<Line>,
    newline: &'static str,
}

/// A key's value as lines: those after the key's own, while indented under
/// it (a list's items may stand at the key's own indentation).
#[derive(Clone, Copy, Debug)]
struct Region {
    /// The key's line, none for the whole file.
    key: Option<usize>,
    /// The lines of its value, `first..end`.
    first: usize,
    end: usize,
}

impl<'a> Outline<'a> {
    fn of(text: &'a str) -> Self {
        let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
        let mut lines = Vec::new();
        let mut start = 0;
        while start < text.len() {
            let end = text[start..].find('\n').map_or(text.len(), |at| start + at + 1);
            let stop = if text[start..end].ends_with("\r\n") {
                end - 2
            } else if text[start..end].ends_with('\n') {
                end - 1
            } else {
                end
            };
            let raw = &text[start..stop];
            let body = raw.trim_start_matches(' ');
            let indent = raw.len() - body.len();
            let kind = classify(body, start + indent);
            lines.push(Line { start, end, stop, indent, kind });
            start = end;
        }
        Self { text, lines, newline }
    }

    fn line_text(&self, index: usize) -> &str {
        let line = &self.lines[index];
        &self.text[line.start..line.stop]
    }

    /// The region of the value at the key path `at`; the whole file for none.
    fn region(&self, at: &[&str]) -> Option<Region> {
        if at.is_empty() {
            return Some(Region { key: None, first: 0, end: self.lines.len() });
        }
        // The keys and list items enclosing each line, by indentation.
        let mut stack: Vec<(usize, Option<String>)> = Vec::new();
        for (index, line) in self.lines.iter().enumerate() {
            if !line.content() {
                continue;
            }
            while let Some((indent, segment)) = stack.last() {
                let item_of_key = line.kind == Kind::Item && *indent == line.indent && segment.is_some();
                if *indent > line.indent || (*indent == line.indent && !item_of_key) {
                    stack.pop();
                } else {
                    break;
                }
            }
            match &line.kind {
                Kind::Key { name, .. } => {
                    stack.push((line.indent, Some(name.clone())));
                    let path: Option<Vec<&str>> = stack.iter().map(|(_, segment)| segment.as_deref()).collect();
                    if path.is_some_and(|path| path == at) {
                        return Some(Region { key: Some(index), first: index + 1, end: self.end_of(index) });
                    }
                }
                Kind::Item => stack.push((line.indent, None)),
                _ => {}
            }
        }
        None
    }

    /// Where the value of the key on line `key` ends: the first content line
    /// after it standing at or left of the key, other than one of its list's
    /// items at the key's own indentation.
    fn end_of(&self, key: usize) -> usize {
        let indent = self.lines[key].indent;
        for (index, line) in self.lines.iter().enumerate().skip(key + 1) {
            if !line.content() {
                continue;
            }
            if line.indent < indent || (line.indent == indent && line.kind != Kind::Item) {
                return index;
            }
        }
        self.lines.len()
    }

    /// The last content line in `first..end`.
    fn last_content(&self, first: usize, end: usize) -> Option<usize> {
        (first..end).rev().find(|&index| self.lines[index].content())
    }

    /// The first content line in `first..end`.
    fn first_content(&self, first: usize, end: usize) -> Option<usize> {
        (first..end).find(|&index| self.lines[index].content())
    }

    /// `text` with the lines `insert` put in at `offset`. After a last line
    /// with no line break the break goes first and the lines end without
    /// one, as the file did.
    fn with(&self, offset: usize, insert: &str) -> String {
        let mut next = String::with_capacity(self.text.len() + insert.len() + 2);
        next.push_str(&self.text[..offset]);
        if offset > 0 && !self.text[..offset].ends_with('\n') {
            next.push_str(self.newline);
            next.push_str(insert.strip_suffix(self.newline).unwrap_or(insert));
        } else {
            next.push_str(insert);
        }
        next.push_str(&self.text[offset..]);
        next
    }

    /// `text` without the lines from `from` to `to`; those that ended it
    /// with no line break take the break before them along.
    fn without(&self, from: usize, to: usize) -> String {
        let mut from = from;
        if to == self.text.len() && !self.text.ends_with('\n') {
            let before = &self.text[..from];
            from -= if before.ends_with("\r\n") { 2 } else { usize::from(before.ends_with('\n')) };
        }
        format!("{}{}", &self.text[..from], &self.text[to..])
    }

    /// Adds `name: value` inside the mapping at `at` (the whole file for
    /// none), first the mappings of `make` that are not there; with what
    /// stood on the key's line for an empty mapping, where block lines
    /// replaced it.
    fn insert_entry(
        &self,
        at: &[&str],
        make: &[&str],
        name: &str,
        value: &Json,
        place: &str,
    ) -> Result<(String, Option<String>), Error> {
        let region = self.region(at).ok_or_else(|| Error::at(place, "cannot be found in the file's outline"))?;
        if let Some(key) = region.key
            && let Kind::Key { inline, value_at, colon_at, .. } = &self.lines[key].kind
            && !inline.is_empty()
        {
            // A mapping written as `{}` or `null` on its key's line takes
            // block lines instead; any other written there is refused.
            if !matches!(inline.as_str(), "{}" | "null" | "~") {
                return Err(Error::at(place, "is written on one line; Fastctx cannot add to it"));
            }
            let end = *value_at + inline.len();
            let emptied = self.text[*colon_at..end].to_owned();
            let bare = format!("{}{}", &self.text[..*colon_at], &self.text[end..]);
            let (next, _) = Outline::of(&bare).insert_entry(at, make, name, value, place)?;
            return Ok((next, Some(emptied)));
        }
        let mut entry = serde_json::Map::new();
        entry.insert(name.to_owned(), value.clone());
        let value = make.iter().rev().fold(Json::Object(entry), |inner, key| {
            let mut outer = serde_json::Map::new();
            outer.insert((*key).to_owned(), inner);
            Json::Object(outer)
        });
        let indent = match self.first_content(region.first, region.end) {
            Some(first) if self.lines[first].kind != Kind::Item => self.lines[first].indent,
            Some(_) => return Err(Error::at(place, "is a list, not a mapping")),
            None => region.key.map_or(0, |key| self.lines[key].indent + 2),
        };
        let offset = match self.last_content(region.first, region.end).or(region.key) {
            Some(line) => self.lines[line].end,
            None => self.text.len(),
        };
        Ok((self.with(offset, &block(&value, indent, self.newline)?), None))
    }

    /// Takes the entry `name` out of the mapping at `at` with all its lines,
    /// then the `made` mappings around it it leaves empty.
    fn remove_entry(&self, at: &[&str], name: &str, made: usize, place: &str) -> Result<String, Error> {
        let mut path: Vec<&str> = at.to_vec();
        path.push(name);
        let region = self.region(&path).ok_or_else(|| Error::at(place, "cannot be found in the file's outline"))?;
        let key = region.key.expect("a key's region");
        let last = self.last_content(region.first, region.end).unwrap_or(key);
        let next = self.without(self.lines[key].start, self.lines[last].end);
        if made == 0 || at.is_empty() {
            return Ok(next);
        }
        // The mapping it was in goes too where Fastctx made it and nothing is
        // left in it.
        let outline = Outline::of(&next);
        let Some(parent) = outline.region(at) else { return Ok(next) };
        if outline.first_content(parent.first, parent.end).is_some() {
            return Ok(next);
        }
        let (name, up) = at.split_last().expect("not empty");
        outline.remove_entry(up, name, made - 1, place)
    }

    /// The text of the setting at `at`.`name`, on its key's line.
    fn scalar_text(&self, at: &[&str], name: &str, place: &str) -> Result<String, Error> {
        let mut path: Vec<&str> = at.to_vec();
        path.push(name);
        let region = self.region(&path).ok_or_else(|| Error::at(place, "cannot be found in the file's outline"))?;
        let key = region.key.expect("a key's region");
        let Kind::Key { inline, .. } = &self.lines[key].kind else { unreachable!() };
        if inline.is_empty() || self.first_content(region.first, region.end).is_some() {
            return Err(Error::at(place, "is not a single value on its key's line"));
        }
        Ok(inline.clone())
    }

    /// The setting at `at`.`name` written as `value`, on its key's line.
    fn replace_scalar(&self, at: &[&str], name: &str, value: &str, place: &str) -> Result<String, Error> {
        let mut path: Vec<&str> = at.to_vec();
        path.push(name);
        let region = self.region(&path).ok_or_else(|| Error::at(place, "cannot be found in the file's outline"))?;
        let key = region.key.expect("a key's region");
        let Kind::Key { inline, value_at, .. } = &self.lines[key].kind else { unreachable!() };
        if inline.is_empty() || self.first_content(region.first, region.end).is_some() {
            return Err(Error::at(place, "is not a single value on its key's line"));
        }
        Ok(format!("{}{value}{}", &self.text[..*value_at], &self.text[*value_at + inline.len()..]))
    }

    /// Adds `item` to the list at `at` (the whole file for none), making the
    /// list and the mappings on its way where they are not there; with the
    /// key and what stood on its line for an empty value, where block lines
    /// replaced it.
    fn append_item(
        &self,
        at: &[&str],
        item: &Json,
        settings: &Json,
        place: &str,
    ) -> Result<(String, Option<Emptied>), Error> {
        let present = at_path(settings, at);
        if at.is_empty() {
            return match present {
                Some(Json::Array(_)) if self.flow_root().is_some() => Ok((self.append_flow_root(item, place)?, None)),
                Some(Json::Array(_)) | Some(Json::Null) | None => Ok((self.append_block(None, item, place)?, None)),
                Some(_) => Err(Error::at(place, "is not a list")),
            };
        }
        match present {
            None => {
                // The list is made, inside the mappings it needs.
                let mut deepest = 0;
                let mut value = settings;
                for key in &at[..at.len() - 1] {
                    match value.get(*key) {
                        Some(inner) if inner.is_object() => {
                            deepest += 1;
                            value = inner;
                        }
                        // There with nothing in it: the rest goes inside.
                        Some(Json::Null) => {
                            deepest += 1;
                            break;
                        }
                        _ => break,
                    }
                }
                let (name, parent) = at.split_last().expect("not empty");
                let list = Json::Array(vec![item.clone()]);
                let (next, emptied) = self.insert_entry(&parent[..deepest], &parent[deepest..], name, &list, place)?;
                Ok((next, emptied.map(|text| (owned(&parent[..deepest]), text))))
            }
            Some(Json::Null) | Some(Json::Array(_)) => {
                let region =
                    self.region(at).ok_or_else(|| Error::at(place, "cannot be found in the file's outline"))?;
                let key = region.key.expect("a key's region");
                let Kind::Key { inline, value_at, colon_at, .. } = &self.lines[key].kind else { unreachable!() };
                if inline.is_empty() || matches!(inline.as_str(), "null" | "~") {
                    let end = *value_at + inline.len();
                    let emptied = (!inline.is_empty()).then(|| (owned(at), self.text[*colon_at..end].to_owned()));
                    let text = format!("{}{}", &self.text[..*colon_at], &self.text[end..]);
                    let text = if inline.is_empty() { self.text.to_owned() } else { text };
                    let outline = Outline::of(&text);
                    let region =
                        outline.region(at).ok_or_else(|| Error::at(place, "cannot be found in the file's outline"))?;
                    return Ok((outline.append_block(Some(region), item, place)?, emptied));
                }
                // A list written on its key's line takes the item there.
                let inline = inline.trim_end();
                if !(inline.starts_with('[') && inline.ends_with(']')) {
                    return Err(Error::at(place, "is not a list Fastctx can add to"));
                }
                let raw = serde_json::to_string(item).expect("a value serializes");
                let close = *value_at + inline.len() - 1;
                let inside = inline[1..inline.len() - 1].trim();
                let insert = if inside.is_empty() { raw } else { format!(", {raw}") };
                let before = self.text[..close].trim_end_matches(' ');
                Ok((format!("{before}{insert}{}", &self.text[close..]), None))
            }
            Some(_) => Err(Error::at(place, "is not a list")),
        }
    }

    /// Adds `item` after the last of the block list in `region` (the whole
    /// file for none).
    fn append_block(&self, region: Option<Region>, item: &Json, place: &str) -> Result<String, Error> {
        let region = region.unwrap_or(Region { key: None, first: 0, end: self.lines.len() });
        let indent = match self.first_content(region.first, region.end) {
            Some(first) if self.lines[first].kind == Kind::Item => self.lines[first].indent,
            Some(_) => return Err(Error::at(place, "is not a block list")),
            None => region.key.map_or(0, |key| self.lines[key].indent + 2),
        };
        let offset = match self.last_content(region.first, region.end).or(region.key) {
            Some(line) => self.lines[line].end,
            None => self.text.len(),
        };
        Ok(self.with(offset, &block(&Json::Array(vec![item.clone()]), indent, self.newline)?))
    }

    /// Where the file is one list written in brackets: its closing bracket.
    fn flow_root(&self) -> Option<usize> {
        let first = self.first_content(0, self.lines.len())?;
        if !self.line_text(first).trim_start().starts_with('[') {
            return None;
        }
        let last = self.last_content(0, self.lines.len())?;
        let line = self.line_text(last);
        let close = line.trim_end().strip_suffix(']').map(|before| before.len())?;
        Some(self.lines[last].start + close)
    }

    /// Adds `item` before the closing bracket of the list the file is.
    fn append_flow_root(&self, item: &Json, place: &str) -> Result<String, Error> {
        let close = self.flow_root().ok_or_else(|| Error::at(place, "is not a list in brackets"))?;
        let open = self.text.find('[').expect("a list in brackets opens");
        let inside = self.text[open + 1..close].trim();
        let raw = serde_json::to_string_pretty(item).expect("a value serializes");
        if inside.is_empty() {
            let lines: String = raw.lines().map(|line| format!("  {line}{}", self.newline)).collect();
            return Ok(format!("{}[{}{lines}]{}", &self.text[..open], self.newline, &self.text[close + 1..]));
        }
        // After the last item, lined up with the first.
        let first = open + 1 + (self.text[open + 1..].len() - self.text[open + 1..].trim_start().len());
        let column = first - self.text[..first].rfind('\n').map_or(0, |at| at + 1);
        let pad = " ".repeat(column);
        let end = open + 1 + self.text[open + 1..close].trim_end().len();
        let lines: Vec<String> = raw.lines().map(|line| format!("{pad}{line}")).collect();
        Ok(format!("{},{}{}{}", &self.text[..end], self.newline, lines.join(self.newline), &self.text[end..]))
    }

    /// Takes the item equal to `item` out of the list at `at`.
    fn remove_item(&self, at: &[&str], item: &Json, place: &str) -> Result<String, Error> {
        let region = self.region(at).ok_or_else(|| Error::at(place, "cannot be found in the file's outline"))?;
        if at.is_empty() && self.flow_root().is_some() {
            return self.remove_flow_item(item, place);
        }
        if let Some(key) = region.key
            && let Kind::Key { inline, value_at, .. } = &self.lines[key].kind
            && !inline.is_empty()
        {
            return self.remove_inline_item(*value_at, inline, item, place);
        }
        // Each item: its `- ` line and those indented under it.
        let Some(first) = self.first_content(region.first, region.end) else {
            return Err(Error::at(place, "holds no item"));
        };
        let indent = self.lines[first].indent;
        let starts: Vec<usize> = (region.first..region.end)
            .filter(|&index| self.lines[index].kind == Kind::Item && self.lines[index].indent == indent)
            .collect();
        for (number, &start) in starts.iter().enumerate() {
            let stop = starts.get(number + 1).copied().unwrap_or(region.end);
            let last = self.last_content(start, stop).unwrap_or(start);
            let lines: String = (start..=last)
                .map(|index| {
                    let text = &self.text[self.lines[index].start..self.lines[index].stop];
                    let cut = text.len().min(indent);
                    let lead = text[..cut].trim_start_matches(' ').is_empty();
                    format!("{}\n", if lead { &text[cut..] } else { text })
                })
                .collect();
            let parsed: Json = serde_yaml_ng::from_str(&lines).unwrap_or(Json::Null);
            if parsed.as_array().is_some_and(|items| items.len() == 1 && items[0] == *item) {
                return Ok(self.without(self.lines[start].start, self.lines[last].end));
            }
        }
        Err(Error::at(place, "the item cannot be found in the file's outline"))
    }

    fn remove_inline_item(&self, value_at: usize, inline: &str, item: &Json, place: &str) -> Result<String, Error> {
        let raw = serde_json::to_string(item).expect("a value serializes");
        let line = &self.text[value_at..value_at + inline.len()];
        for candidate in [format!(", {raw}"), format!("{raw}, "), raw.clone()] {
            if let Some(at) = line.find(&candidate) {
                let start = value_at + at;
                return Ok(self.without(start, start + candidate.len()));
            }
        }
        Err(Error::at(place, "the item cannot be found as Fastctx wrote it"))
    }

    fn remove_flow_item(&self, item: &Json, place: &str) -> Result<String, Error> {
        let raw = serde_json::to_string_pretty(item).expect("a value serializes");
        let close = self.flow_root().expect("a list in brackets");
        let open = self.text.find('[').expect("a list in brackets opens");
        let inside = &self.text[open + 1..close];
        let lines: Vec<&str> = raw.lines().collect();
        // As Fastctx wrote it: lined up under the others, after a comma, or
        // alone in the brackets.
        for (start, _) in inside.match_indices(lines[0]) {
            let begin = open + 1 + start;
            let mut end = begin;
            let mut fits = true;
            for (number, line) in lines.iter().enumerate() {
                let line = line.trim_start();
                let rest = &self.text[end..];
                let rest = if number == 0 { rest } else { rest.trim_start_matches(['\r', '\n', ' ']) };
                if !rest.starts_with(line) {
                    fits = false;
                    break;
                }
                end = self.text.len() - rest.len() + line.len();
            }
            if !fits {
                continue;
            }
            let before = self.text[open + 1..begin].trim_end();
            let after = self.text[end..close].trim();
            return Ok(if before.is_empty() && after.is_empty() {
                format!("{}[]{}", &self.text[..open], &self.text[close + 1..])
            } else if let Some(comma) = before.strip_suffix(',') {
                let cut = open + 1 + comma.len();
                format!("{}{}", &self.text[..cut], &self.text[end..])
            } else {
                let rest = self.text[end..].trim_start_matches([' ', '\r', '\n']);
                let rest = rest.strip_prefix(',').unwrap_or(rest).trim_start_matches([' ', '\r', '\n']);
                format!("{}{}", &self.text[..begin], rest)
            });
        }
        Err(Error::at(place, "the item cannot be found as Fastctx wrote it"))
    }
}

/// What a line holds, its indentation taken off; `offset` is where its
/// content starts in the file.
fn classify(body: &str, offset: usize) -> Kind {
    if body.is_empty() {
        return Kind::Blank;
    }
    if body.starts_with('#') {
        return Kind::Comment;
    }
    if body == "-" || body.starts_with("- ") {
        return Kind::Item;
    }
    let (name, rest) = if let Some(quote) = body.chars().next().filter(|c| *c == '"' || *c == '\'') {
        let Some(close) = body[1..].find(quote) else { return Kind::Other };
        let name = &body[1..=close];
        (name.to_owned(), &body[close + 2..])
    } else {
        if body.starts_with(['[', '{', '&', '*', '!', '|', '>', '%', '@', '`', '?']) {
            return Kind::Other;
        }
        let Some(colon) = body.find(": ").or_else(|| body.strip_suffix(':').map(str::len)) else {
            return Kind::Other;
        };
        let name = &body[..colon];
        if name.contains(" #") {
            return Kind::Other;
        }
        (name.trim_end().to_owned(), &body[colon..])
    };
    let Some(rest) = rest.strip_prefix(':') else { return Kind::Other };
    if !(rest.is_empty() || rest.starts_with(' ')) {
        return Kind::Other;
    }
    let spaced = rest.len() - rest.trim_start().len();
    let value = rest.trim_start();
    // A comment after the value is not part of it.
    let value = match value.find(" #") {
        Some(at) if !value.starts_with(['"', '\'']) => &value[..at],
        _ if value.starts_with('#') => "",
        _ => value,
    };
    let colon_at = offset + (body.len() - rest.len());
    Kind::Key { name, inline: value.trim_end().to_owned(), colon_at, value_at: colon_at + spaced }
}
