//! A streaming, atomic import of the pre-SQLite records. Every source is
//! read a second time and every row compared before the transaction commits.
//! Renaming the sources is restartable and happens only after that commit.

use super::shelf::{self, Shelf};
use fastctx_storage::{
    Error,
    rusqlite::{Connection, params},
};
use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde_json::Value;
use std::{
    collections::HashMap,
    fmt,
    fs::File,
    io::{self, BufReader},
    path::Path,
};

const VERSION: u64 = 1;
const SOURCES: &[(&str, Route)] = &[
    ("directory.json", Route::Book),
    ("hub.json", Route::Hub),
    ("letters.json", Route::Map("letters")),
    ("worked.json", Route::List("worked")),
    ("updates/reports.json", Route::Map("reports")),
];

#[derive(Clone, Copy)]
enum Route {
    Book,
    Hub,
    History,
    Forgetting,
    Map(&'static str),
    List(&'static str),
}

pub(super) fn import(c: &Connection, dir: &Path) -> fastctx_storage::Result<()> {
    match shelf::meta::<u64>(c, "format")? {
        Some(VERSION) => return Ok(()),
        Some(other) => return Err(Error::new(format!("hub database format {other} is not supported"))),
        None => {}
    }
    // A missing database must never make the retained sources into a new hub.
    for (name, _) in SOURCES {
        if dir.join(backup_name(name)).try_exists()? {
            return Err(Error::new(
                "a pre-SQLite backup exists but its database is missing; restore the complete hub backup",
            ));
        }
    }
    let mut sources = Vec::new();
    for &(name, route) in SOURCES {
        if !dir.join(name).try_exists()? {
            if matches!(route, Route::Book | Route::Hub) {
                return Err(Error::new(format!("the hub's {name} is missing")));
            }
            continue;
        }
        walk(c, &dir.join(name), route, false)?;
        sources.push(name);
    }
    for &(name, route) in SOURCES {
        if sources.contains(&name) {
            walk(c, &dir.join(name), route, true)?;
        }
    }
    super::legacy::normalize(c)?;
    shelf::put_meta(c, "legacy_sources", &serde_json::to_vec(&sources).map_err(error)?)?;
    shelf::put_meta(c, "format", b"1")
}

fn walk(c: &Connection, path: &Path, route: Route, verify: bool) -> fastctx_storage::Result<()> {
    let mut decoder = serde_json::Deserializer::from_reader(BufReader::with_capacity(64 * 1024, File::open(path)?));
    Walk { c, route, verify }
        .deserialize(&mut decoder)
        .map_err(|e| Error::new(format!("reading {}: {e}", path.display())))?;
    decoder.end().map_err(error)
}

fn error(e: impl std::fmt::Display) -> Error {
    Error::new(e.to_string())
}

struct Walk<'a> {
    c: &'a Connection,
    route: Route,
    verify: bool,
}
impl<'de> DeserializeSeed<'de> for Walk<'_> {
    type Value = ();
    fn deserialize<D: de::Deserializer<'de>>(self, d: D) -> Result<(), D::Error> {
        match self.route {
            Route::List(_) => d.deserialize_seq(self),
            _ => d.deserialize_map(self),
        }
    }
}
impl<'de> Visitor<'de> for Walk<'_> {
    type Value = ();
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("hub records")
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
        let Route::List(kind) = self.route else { return Err(de::Error::custom("unexpected array")) };
        let mut seen = HashMap::<String, usize>::new();
        let mut position = 0;
        while let Some(value) = seq.next_element::<Value>()? {
            let base = key(kind, &value);
            let key = if kind == "machine_log" {
                base
            } else {
                let occurrence = seen.entry(base.clone()).or_default();
                *occurrence += 1;
                if *occurrence == 1 { base } else { format!("{base}#{occurrence}") }
            };
            row(self.c, kind, &key, position, &value, self.verify).map_err(de::Error::custom)?;
            position += 1;
        }
        if self.verify {
            let actual: i64 = if kind == "machine_log" {
                self.c.query_row("SELECT count(*) FROM machine_log", [], |r| r.get(0))
            } else {
                self.c.query_row("SELECT count(*) FROM hub_rows WHERE kind=?1", [kind], |r| r.get(0))
            }
            .map_err(de::Error::custom)?;
            if actual != position {
                return Err(de::Error::custom(format!("{kind} row count differs")));
            }
        }
        Ok(())
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        let mut count = 0i64;
        let mut fields = std::collections::HashSet::new();
        while let Some(name) = map.next_key::<String>()? {
            if !matches!(self.route, Route::Map(_)) && !fields.insert(name.clone()) {
                return Err(de::Error::custom("a hub record contains a duplicate field"));
            }
            let nested = match (self.route, name.as_str()) {
                (Route::Book, "accounts") => Some(Route::List("accounts")),
                (Route::Book, "members") => Some(Route::List("members")),
                (Route::Book, "shares") => Some(Route::List("shares")),
                (Route::Book, "lends") => Some(Route::List("lends")),
                (Route::Book, "plans") => Some(Route::List("plans")),
                (Route::Book, "acting") => Some(Route::List("acting")),
                (Route::Book, "projects") => Some(Route::List("projects")),
                (Route::Book, "passwords") => Some(Route::Map("passwords")),
                (Route::Book, "tokens") => Some(Route::List("tokens")),
                (Route::Book, "removed") => Some(Route::List("removed")),
                (Route::Book, "archived") => Some(Route::List("archived")),
                (Route::Book, "project_deletions") => Some(Route::List("project_deletions")),
                (Route::Book, "forgetting") => Some(Route::Forgetting),
                (Route::Book, "machine_log") => Some(Route::History),
                (Route::History, "records") => Some(Route::List("machine_log")),
                (Route::Forgetting, "pending") => Some(Route::List("forgetting")),
                (Route::Hub, "due") => Some(Route::List("due")),
                (Route::Hub, "repairs") => Some(Route::List("repairs")),
                (Route::Hub, "package_targets") => Some(Route::Map("package_targets")),
                _ => None,
            };
            if let Some(route) = nested {
                map.next_value_seed(Walk { route, ..self })?;
            } else {
                let value = map.next_value::<Value>()?;
                match self.route {
                    Route::Map(kind) => {
                        super::validate::key(kind, &name).map_err(de::Error::custom)?;
                        row(self.c, kind, &name, 0, &value, self.verify).map_err(de::Error::custom)?;
                    }
                    route => {
                        let prefix = match route {
                            Route::Book => "book",
                            Route::Hub => "hub",
                            Route::History => "machine_log",
                            Route::Forgetting => "forgetting",
                            _ => unreachable!(),
                        };
                        let name = format!("{prefix}.{name}");
                        super::validate::field(&name, &value).map_err(de::Error::custom)?;
                        let bytes = serde_json::to_vec(&value).map_err(de::Error::custom)?;
                        if self.verify {
                            let stored = shelf::meta::<Value>(self.c, &name).map_err(de::Error::custom)?;
                            if stored.as_ref() != Some(&value) {
                                return Err(de::Error::custom(format!("{name} differs")));
                            }
                        } else {
                            shelf::put_meta(self.c, &name, &bytes).map_err(de::Error::custom)?;
                        }
                    }
                }
            }
            count += 1;
        }
        let required: &[&str] = match self.route {
            Route::Book => &["accounts", "members", "projects"],
            Route::Hub => &["id", "name", "public", "listen"],
            _ => &[],
        };
        for name in required {
            if !fields.contains(*name) {
                return Err(de::Error::custom(format!("the hub's {name} is missing")));
            }
        }
        if self.verify
            && let Route::Map(kind) = self.route
        {
            let actual: i64 = self
                .c
                .query_row("SELECT count(*) FROM hub_rows WHERE kind=?1", [kind], |r| r.get(0))
                .map_err(de::Error::custom)?;
            if actual != count {
                return Err(de::Error::custom(format!("{kind} row count differs")));
            }
        }
        Ok(())
    }
}

/// The same identity functions are used by live list diffs and the importer.
pub(super) fn key(kind: &str, value: &Value) -> String {
    let field = |name: &str| value.get(name).cloned().unwrap_or(Value::Null);
    let parts = |names: &[&str]| Value::Array(names.iter().map(|n| field(n)).collect());
    let key = match kind {
        "accounts" => field("name"),
        "members" | "tokens" | "removed" | "due" | "repairs" => field("key"),
        "projects" | "archived" => field("town"),
        "shares" | "acting" => parts(&["machine", "account"]),
        "lends" => parts(&["machine", "account", "agent", "home"]),
        "plans" => parts(&["machine", "agent", "home"]),
        "project_deletions" => value["project"]["town"].clone(),
        "forgetting" => value["forgetting"]["id"].clone(),
        "machine_log" => value["record"]["id"].clone(),
        _ => value.clone(),
    };
    key.to_string()
}

fn row(
    c: &Connection,
    kind: &str,
    key: &str,
    position: i64,
    value: &Value,
    verify: bool,
) -> fastctx_storage::Result<()> {
    super::validate::row(kind, value)?;
    let bytes = serde_json::to_vec(value).map_err(error)?;
    if verify {
        let stored: Vec<u8> = if kind == "machine_log" {
            let id = value["record"]["id"].as_u64().ok_or_else(|| Error::new("machine log id is out of range"))?;
            let id = super::log_store::sql_id(id);
            c.prepare_cached("SELECT record FROM machine_log WHERE id=?1")?.query_row([id], |r| r.get(0))?
        } else {
            let (at, stored): (i64, Vec<u8>) = c
                .prepare_cached("SELECT position,body FROM hub_rows WHERE kind=?1 AND key=?2")?
                .query_row(params![kind, key], |r| Ok((r.get(0)?, r.get(1)?)))?;
            if at != position {
                return Err(Error::new(format!("{kind} row position differs")));
            }
            stored
        };
        if shelf::decode::<Value>(&stored)? != *value {
            return Err(Error::new(format!("{kind} row content differs")));
        }
        return Ok(());
    }
    if kind == "machine_log" {
        let row = super::log_store::Row::from_bytes(bytes)?;
        super::log_store::put(c, &row)
    } else {
        shelf::put(c, kind, key, position, &bytes)
    }
}

fn backup_name(name: &str) -> String {
    format!("{}.before-sqlite.json", name.strip_suffix(".json").unwrap_or(name))
}

pub(super) fn back_up(shelf: &Shelf, dir: &Path) -> io::Result<()> {
    if shelf.meta::<bool>("legacy_backed_up")?.unwrap_or(false) {
        return Ok(());
    }
    let sources = shelf.meta::<Vec<String>>("legacy_sources")?.unwrap_or_default();
    for name in sources.iter().filter(|n| n.as_str() != "hub.json") {
        let source = dir.join(name);
        if !source.try_exists()? {
            continue;
        }
        let backup = dir.join(backup_name(name));
        if backup.try_exists()? {
            return Err(io::Error::other("both a legacy source and its SQLite migration backup exist"));
        }
        std::fs::rename(source, backup)?;
    }
    let hub = dir.join("hub.json");
    let backup = dir.join(backup_name("hub.json"));
    if !backup.try_exists()? {
        std::fs::rename(&hub, &backup)?;
    }
    let mut identity = serde_json::Map::new();
    for field in ["id", "name", "public", "listen"] {
        let value = shelf
            .meta::<Value>(&format!("hub.{field}"))?
            .ok_or_else(|| io::Error::other("hub identity missing from migration"))?;
        identity.insert(field.into(), value);
    }
    fastctx_platform::publish(&hub, &serde_json::to_vec(&identity)?, fastctx_platform::Permissions::Private)?;
    shelf.0.write_now(0, |c| shelf::put_meta(c, "legacy_backed_up", b"true")).map_err(shelf::other)
}
