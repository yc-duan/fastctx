//! A conversation's records the hub orders — its plan among them — read
//! from a copy of its scope (plan 3-5, design §28.27, §28.39): the hub
//! orders each into the conversation's scope as its account record, so a
//! machine or a phone holding the conversation holds them as the hub last
//! decided, and reads them the same way, from this one place, while the
//! hub is not reached.

use fastctx_content::v2::{Entry, Library};
use fastctx_net::directory::Directory;
use fastctx_types::account_records::Record;
use fastctx_types::plan::{Author, Body, KIND, Page};
use fastctx_types::{AccountName, MachineName, ResidentId};

/// An item's whole body as written: its body, or the one object it is in
/// when it was too large to be one (`{"in_object":true,…}`, AR03, M17).
pub fn whole_body(library: &Library, entry: &Entry) -> Option<Vec<u8>> {
    let value: serde_json::Value = serde_json::from_slice(&entry.body).ok()?;
    if value.get("in_object").and_then(serde_json::Value::as_bool) != Some(true) {
        return Some(entry.body.clone());
    }
    let [object] = entry.objects.as_slice() else { return None };
    object_bytes(library, object)
}

/// The bytes of `object`, whole; none when they are not all here.
pub fn object_bytes(library: &Library, object: &fastctx_content::ObjectRef) -> Option<Vec<u8>> {
    let (mut bytes, mut at) = (Vec::new(), 0u64);
    while at < object.size {
        let piece = library.read(&object.hash, at, 1 << 20).ok()?;
        if piece.is_empty() {
            return None;
        }
        at += piece.len() as u64;
        bytes.extend_from_slice(&piece);
    }
    Some(bytes)
}

/// The account record `item` of `stream` in the copy `scope`: none when it
/// cannot be read, `Some(None)` when the copy holds no such record.
pub fn record_in(library: &Library, scope: &str, stream: &str, item: &str) -> Option<Option<Record>> {
    let item = fastctx_content::v2::item_of(item);
    let Some(entry) = library.item(scope, stream, &item).ok()? else { return Some(None) };
    let body = whole_body(library, &entry)?;
    Some(Some(serde_json::from_slice(&body).ok()?))
}

/// The plan of the conversation `resident` as the copy's record `record`
/// holds it, shown to `account` as the hub shows it (`Link::plan_read`,
/// `hub owned_records::projection`): which machine wrote it only where the
/// account may work on that machine, by `directory` as it stands.
pub fn plan_as_told(directory: &Directory, account: &AccountName, resident: &ResidentId, record: Option<Record>) -> Page {
    let record = record.filter(|record| record.metadata.kind == KIND && record.metadata.key == resident.as_str());
    let Some(mut record) = record else { return Page { owner: resident.clone(), metadata: None, body: None } };
    let usable = |machine: &MachineName| directory.authorize(account, fastctx_net::authorization::Action::Work(machine)).is_ok();
    if record.metadata.by.as_ref().is_some_and(|machine| !usable(machine)) {
        record.metadata.by = None;
    }
    let mut body: Option<Body> = if record.metadata.deleted { None } else { serde_json::from_value(record.body).ok() };
    if let Some(body) = &mut body
        && matches!(&body.author, Author::Resident { address } if !usable(&address.machine))
    {
        body.author = Author::Elsewhere;
    }
    Page { owner: resident.clone(), metadata: Some(record.metadata), body }
}
