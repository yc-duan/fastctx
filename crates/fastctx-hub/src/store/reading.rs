//! Offline inspection never runs startup recovery or changes letter presence.
use super::{HubFile, Store, shelf};
use fastctx_storage::rusqlite::{Connection, OpenFlags};
use serde::de::DeserializeOwned;
use std::{io, path::Path};

fn open(dir: &Path) -> io::Result<Option<Connection>> {
    let path = dir.join(shelf::FILE);
    if !path.try_exists()? {
        return Ok(None);
    }
    let c = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(io::Error::other)?;
    c.busy_timeout(std::time::Duration::from_secs(5)).map_err(io::Error::other)?;
    if shelf::meta::<u64>(&c, "format").map_err(shelf::other)? != Some(1) {
        return Err(io::Error::other("the hub database import has not committed"));
    }
    Ok(Some(c))
}
fn list<T: DeserializeOwned>(c: &Connection, kind: &str) -> io::Result<Vec<T>> {
    let mut stmt =
        c.prepare("SELECT body FROM hub_rows WHERE kind=?1 ORDER BY position,key").map_err(io::Error::other)?;
    let mut cursor = stmt.query([kind]).map_err(io::Error::other)?;
    let mut out = Vec::new();
    while let Some(row) = cursor.next().map_err(io::Error::other)? {
        out.push(shelf::decode(&row.get::<_, Vec<u8>>(0).map_err(io::Error::other)?).map_err(shelf::other)?);
    }
    Ok(out)
}
pub(super) fn hub(dir: &Path) -> io::Result<HubFile> {
    let file = Store::hub_file(dir);
    let file = if file.try_exists()? { file } else { dir.join("hub.before-sqlite.json") };
    let mut hub: HubFile = super::read(&file)?;
    let Some(mut c) = open(dir)? else { return Ok(hub) };
    let tx = c.transaction().map_err(io::Error::other)?;
    hub.self_registration =
        shelf::meta(&tx, "hub.self_registration").map_err(shelf::other)?.unwrap_or(hub.self_registration);
    hub.updates = shelf::meta(&tx, "hub.updates").map_err(shelf::other)?.unwrap_or(hub.updates);
    hub.dissolved = shelf::meta(&tx, "hub.dissolved").map_err(shelf::other)?.unwrap_or(hub.dissolved);
    hub.due = list(&tx, "due")?;
    hub.repairs = list(&tx, "repairs")?;
    let mut stmt =
        tx.prepare("SELECT key,body FROM hub_rows WHERE kind='package_targets'").map_err(io::Error::other)?;
    let mut cursor = stmt.query([]).map_err(io::Error::other)?;
    hub.package_targets.clear();
    while let Some(row) = cursor.next().map_err(io::Error::other)? {
        let key: String = row.get(0).map_err(io::Error::other)?;
        let value: Vec<u8> = row.get(1).map_err(io::Error::other)?;
        hub.package_targets
            .insert(key.parse().map_err(io::Error::other)?, shelf::decode(&value).map_err(shelf::other)?);
    }
    Ok(hub)
}

pub(super) fn directory(dir: &Path) -> io::Result<fastctx_net::directory::Directory> {
    let Some(mut c) = open(dir)? else {
        // Reading only: older account shapes are interpreted without rewriting
        // the source while a move is still comparing its file digest.
        let raw: serde_json::Value = super::read(&Store::directory_file(dir))?;
        let (mut raw, _) = crate::upgrade::upgrade(raw);
        crate::upgrade::drop_relays(&mut raw);
        return serde_json::from_value::<super::Book>(raw).map(|book| book.directory).map_err(io::Error::other);
    };
    let tx = c.transaction().map_err(io::Error::other)?;
    Ok(fastctx_net::directory::Directory {
        accounts: list(&tx, "accounts")?,
        members: list(&tx, "members")?,
        shares: list(&tx, "shares")?,
        assists: list(&tx, "lends")?,
        plans: list(&tx, "plans")?,
        acting: list(&tx, "acting")?,
        projects: list(&tx, "projects")?,
    })
}
