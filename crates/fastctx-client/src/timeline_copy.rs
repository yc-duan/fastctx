//! A conversation's timeline read from a copy of its asset scope (design
//! §28.30, §28.39): a page of it, what it changed to since a reading,
//! whether it moved, and the bytes of an object it refers to. A machine
//! holding another's conversation and a phone holding a replica read it
//! the same way, from this one place. A place read from one source means
//! nothing to another: what the machine the conversation lives on tells is
//! marked (`ELSEWHERE`), and a reader whose place is of the other source is
//! told to read anew.

use fastctx_content::v2::{
    ConversationFileChange, ConversationMeta, EVENT, HeldScope, Kind as _, Library, OVERSIZED, Timeline, ViewChange,
    view_sequence,
};
use fastctx_content::{Anchor, ObjectRef, Place};
use fastctx_net::directory::Directory;
use fastctx_types::assets::{Descriptor, Holders, Readers, conversation_classes};
use fastctx_types::ipc::Failure;
use fastctx_types::timeline_pages::{
    FILES_ITEMS, FileChangeTold, OBJECT_BYTES, TimelineAsk, TimelineChange, TimelineEvent, TimelineFrom, TimelinePlace,
    TimelineShown, TimelineTold,
};
use fastctx_types::town::{ResidentNow, WhoReply};
use fastctx_types::{AccountName, MachineName};
use std::time::Duration;

/// The most a page holds, whatever is asked.
pub const PAGE_ITEMS: u32 = 400;
pub const PAGE_BYTES: u32 = 1024 * 1024;

/// The mark of a reader's place read where the conversation lives, not in
/// a copy: the top bit of the revisions and sequences it is told.
pub const ELSEWHERE: u64 = 1 << 63;

/// How long a reader waiting for a copy's timeline to move is kept.
const MOVED_WITHIN: Duration = Duration::from_secs(25);

fn failure(error: impl std::fmt::Display) -> Failure {
    Failure::new(error.to_string())
}

/// What a timeline record shows.
pub enum Shown {
    /// The event, as JSON.
    Event(Vec<u8>),
    /// An event too large for a page: its length as JSON, what is shown in
    /// its place when something is (JSON), and the object that holds it whole.
    Oversized { bytes: u64, slim: Option<Vec<u8>>, object: ObjectRef },
}

/// A timeline record as a reader is told it.
pub struct Entry {
    pub place: Place,
    /// The timeline's count of records as this one was written.
    pub sequence: u64,
    pub shown: Shown,
    pub likeness: Option<[u8; 32]>,
}

/// A page of a timeline.
pub struct Read {
    /// The timeline's revision as the page was read; none while the
    /// conversation's timeline has not been made.
    pub revision: Option<u64>,
    pub entries: Vec<Entry>,
    pub older: Option<Place>,
    pub newer: Option<Place>,
    /// How many records the timeline held as the page was read.
    pub through: u64,
}

pub enum Change {
    /// The event stands at its place as shown from this record on.
    Is(Entry),
    /// The timeline holds the item no more.
    Gone { item: Vec<u8> },
}

/// What a timeline changed to after a reading.
pub struct Changed {
    pub revision: Option<u64>,
    /// In the order they were written.
    pub changes: Vec<Change>,
    /// The count of records these reach to.
    pub through: u64,
    pub more: bool,
}

/// What a timeline record's body shows: the event, or what stands for one
/// kept whole as an object.
pub fn likeness(body: &[u8]) -> Option<[u8; 32]> {
    match body.split_first()? {
        (&EVENT, encoded) => {
            let event = serde_json::from_slice(encoded).ok()?;
            Some(fastctx_types::likeness::likeness(&event, encoded))
        }
        (&OVERSIZED, told) => serde_json::from_slice::<serde_json::Value>(told)
            .ok()
            .and_then(|value| serde_json::from_value(value.get("likeness")?.clone()).ok()),
        _ => None,
    }
}

pub fn shown(body: &[u8], objects: &[ObjectRef]) -> Result<Shown, Failure> {
    match body.split_first() {
        Some((&EVENT, event)) => Ok(Shown::Event(event.to_vec())),
        Some((&OVERSIZED, told)) => {
            let told: serde_json::Value = serde_json::from_slice(told).map_err(failure)?;
            let object = *objects.first().ok_or_else(|| failure("an oversized event names no object"))?;
            let slim = match &told["slim"] {
                serde_json::Value::Null => None,
                slim => Some(serde_json::to_vec(slim).map_err(failure)?),
            };
            Ok(Shown::Oversized { bytes: told["bytes"].as_u64().unwrap_or(object.size), slim, object })
        }
        _ => Err(failure("the timeline holds an item it cannot read")),
    }
}

/// Where a conversation's timeline is read in an asset library.
pub struct Copy {
    library: Library,
    scope: String,
}

impl Copy {
    /// The timeline of `scope` in `library`, read as it stands.
    pub fn at(library: Library, scope: String) -> Self {
        Self { library, scope }
    }

    pub fn library(&self) -> &Library {
        &self.library
    }

    pub fn scope(&self) -> &str {
        &self.scope
    }
}

/// An entry as a reader at `at` knows it: its sequence is the write number
/// it took here, in the era read in, so an entry written again is told
/// again (JJ04, §29 3.3 PH04).
fn entry(at: u64, entry: fastctx_content::v2::Entry) -> Result<Entry, Failure> {
    Ok(Entry {
        shown: shown(&entry.body, &entry.objects)?,
        likeness: likeness(&entry.body),
        place: Place { order: entry.order, item: entry.item },
        sequence: view_sequence(at, entry.local),
    })
}

/// A page of the timeline as the asset library holds it.
pub fn page(copy: &Copy, anchor: &Anchor, max_items: u32, max_bytes: u32) -> Result<Read, Failure> {
    let page = copy.library.view_page(&copy.scope, Timeline::STREAM, anchor, max_items, max_bytes).map_err(failure)?;
    Ok(Read {
        revision: page.revision,
        entries: page.entries.into_iter().map(|found| entry(page.through, found)).collect::<Result<_, Failure>>()?,
        older: page.older,
        newer: page.newer,
        through: page.through,
    })
}

/// What the timeline changed to after `after`, as the asset library holds
/// it; none when the reader reads its pages again.
pub fn changes(copy: &Copy, after: u64, max_items: u32, max_bytes: u32) -> Result<Option<Changed>, Failure> {
    let Some(read) =
        copy.library.view_changes(&copy.scope, Timeline::STREAM, after, max_items, max_bytes).map_err(failure)?
    else {
        return Ok(None);
    };
    let changes = read
        .changes
        .into_iter()
        .map(|change| {
            Ok(match change {
                ViewChange::Is(found) => Change::Is(entry(read.through, found)?),
                ViewChange::Gone { item } => Change::Gone { item },
            })
        })
        .collect::<Result<_, Failure>>()?;
    Ok(Some(Changed { revision: read.revision, changes, through: read.through, more: read.more }))
}

/// Waits until the timeline in the asset library is past `after`.
pub async fn moved(copy: &Copy, after: u64, within: Duration) -> Result<Option<u64>, Failure> {
    copy.library.view_moved(&copy.scope, Timeline::STREAM, after, within).await.map_err(failure)
}

/// A range of an object the timeline refers to; none when it refers to no
/// such object or the object is not here yet.
pub fn object(copy: &Copy, hash: &[u8; 32], offset: u64, length: u32) -> Result<Option<Vec<u8>>, Failure> {
    if !copy.library.refers(&copy.scope, hash).map_err(failure)? || copy.library.size(hash).map_err(failure)?.is_none()
    {
        return Ok(None);
    }
    copy.library.read(hash, offset, length).map(Some).map_err(failure)
}

/// The most bytes of file changes one answer holds.
const FILES_BYTES: u32 = 2 << 20;

/// The file changes of the conversation whose scope is `scope` in `library`
/// (`conversation.files`), those written after `after` — 0 for all — as
/// the machine it lives on wrote them: none yet before the first is written
/// there, or comes here.
pub fn files(library: &Library, scope: &str, after: u64, max_items: u32) -> Result<TimelineTold, Failure> {
    let stream = ConversationFileChange::STREAM;
    let Some(revision) = library.view_revision(scope, stream).map_err(failure)? else {
        return Ok(TimelineTold::Files { known: true, changes: Vec::new(), through: 0, more: false });
    };
    let from = if after == 0 { revision & !((1u64 << 40) - 1) } else { after };
    let Some(read) =
        library.view_changes(scope, stream, from, max_items.clamp(1, FILES_ITEMS), FILES_BYTES).map_err(failure)?
    else {
        return Ok(TimelineTold::Reread);
    };
    let changes = read
        .changes
        .into_iter()
        .map(|change| match change {
            ViewChange::Is(entry) => FileChangeTold {
                item: String::from_utf8_lossy(&entry.item).into_owned(),
                change: serde_json::from_slice(&entry.body).ok(),
            },
            ViewChange::Gone { item } => {
                FileChangeTold { item: String::from_utf8_lossy(&item).into_owned(), change: None }
            }
        })
        .collect();
    Ok(TimelineTold::Files { known: true, changes, through: read.through, more: read.more })
}

/// Answers `request` from the copy. What the conversation runs now only
/// the machine it lives on has.
pub async fn answer(copy: &Copy, request: TimelineAsk) -> Result<TimelineTold, Failure> {
    match request {
        TimelineAsk::Moved { after, .. } => {
            let revision = moved(copy, after, MOVED_WITHIN).await?;
            Ok(TimelineTold::Moved { revision, initializing: Some(false) })
        }
        TimelineAsk::Page { from, max_events, max_bytes, .. } => {
            let anchor = anchor(from)?;
            fastctx_platform::roles::in_place("reading a timeline page of a copy", || {
                let read = page(copy, &anchor, max_events.clamp(1, PAGE_ITEMS), max_bytes.min(PAGE_BYTES))?;
                Ok(TimelineTold::Page {
                    revision: read.revision,
                    initializing: Some(false),
                    events: read.entries.into_iter().map(event).collect::<Result<_, Failure>>()?,
                    older: read.older.map(encode_place),
                    newer: read.newer.map(encode_place),
                    through: Some(read.through),
                })
            })
        }
        TimelineAsk::Changes { after, max_events, max_bytes, .. } => {
            fastctx_platform::roles::in_place("reading a copy's changes", || {
                let Some(read) = changes(copy, after, max_events.clamp(1, PAGE_ITEMS), max_bytes.min(PAGE_BYTES))?
                else {
                    return Ok(TimelineTold::Reread);
                };
                Ok(TimelineTold::Changes {
                    revision: read.revision,
                    initializing: Some(false),
                    changes: read
                        .changes
                        .into_iter()
                        .map(|change| {
                            Ok(match change {
                                Change::Is(entry) => TimelineChange::Is { event: event(entry)? },
                                Change::Gone { item } => TimelineChange::Gone { item: hex(&item) },
                            })
                        })
                        .collect::<Result<_, Failure>>()?,
                    through: read.through,
                    more: read.more,
                })
            })
        }
        TimelineAsk::Object { object: name, offset, length, .. } => {
            let hash: [u8; 32] =
                decode(&name)?.try_into().map_err(|_| Failure::new("a timeline object name must be a SHA-256 hash"))?;
            fastctx_platform::roles::in_place("reading a copy's object", || {
                use base64::Engine as _;
                let bytes = object(copy, &hash, offset, length.min(OBJECT_BYTES))?;
                Ok(TimelineTold::Object {
                    base64: bytes.map(|bytes| base64::engine::general_purpose::STANDARD.encode(bytes)),
                })
            })
        }
        TimelineAsk::Running { .. } => Err(Failure::new("what runs now is read where the conversation lives")),
        TimelineAsk::Files { after, max_items, .. } => {
            fastctx_platform::roles::in_place("reading a copy's file changes", || {
                files(&copy.library, &copy.scope, after, max_items)
            })
        }
    }
}

/// Where a page asked from `from` begins.
pub fn anchor(from: TimelineFrom) -> Result<Anchor, Failure> {
    Ok(match from {
        TimelineFrom::Latest => Anchor::Latest,
        TimelineFrom::Before { place } => Anchor::Before(decode_place(place)?),
        TimelineFrom::After { place } => Anchor::After(decode_place(place)?),
    })
}

/// An event of the timeline as a reader is told it.
pub fn event(entry: Entry) -> Result<TimelineEvent, Failure> {
    let shown = match entry.shown {
        Shown::Event(event) => TimelineShown::Event { event: serde_json::from_slice(&event).map_err(failure)? },
        Shown::Oversized { bytes, slim, object } => TimelineShown::Oversized {
            bytes,
            slim: slim.map(|slim| serde_json::from_slice(&slim)).transpose().map_err(failure)?,
            object: hex(&object.hash),
        },
    };
    Ok(TimelineEvent { place: encode_place(entry.place), sequence: entry.sequence, likeness: entry.likeness, shown })
}

pub fn decode_place(place: TimelinePlace) -> Result<Place, Failure> {
    let item = decode(&place.item)?;
    if item.is_empty() {
        return Err(Failure::new("a timeline item's identity is empty"));
    }
    Ok(Place { order: decode(&place.order)?, item })
}

pub fn encode_place(place: Place) -> TimelinePlace {
    TimelinePlace { order: hex(&place.order), item: hex(&place.item) }
}

pub fn decode(text: &str) -> Result<Vec<u8>, Failure> {
    let digit = |byte| match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    };
    if !text.len().is_multiple_of(2) || text.len() > fastctx_content::KEY_BYTES * 2 {
        return Err(Failure::new("a timeline position or object name has the wrong length"));
    }
    let mut bytes = vec![0; text.len() / 2];
    for (byte, pair) in bytes.iter_mut().zip(text.as_bytes().chunks_exact(2)) {
        let (Some(high), Some(low)) = (digit(pair[0]), digit(pair[1])) else {
            return Err(Failure::new("a timeline position or object name must be lowercase hexadecimal"));
        };
        *byte = high * 16 + low;
    }
    Ok(bytes)
}

pub fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8] = b"0123456789abcdef";
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        text.push(DIGITS[(byte >> 4) as usize] as char);
        text.push(DIGITS[(byte & 15) as usize] as char);
    }
    text
}

/// The place a reader asks after, where it gives one.
pub fn after(request: &TimelineAsk) -> Option<u64> {
    match request {
        TimelineAsk::Moved { after, .. } | TimelineAsk::Changes { after, .. } | TimelineAsk::Files { after, .. } => {
            Some(*after)
        }
        _ => None,
    }
}

/// The answer to a reader whose place is of another source: read anew.
pub fn anew(request: &TimelineAsk) -> TimelineTold {
    match request {
        TimelineAsk::Moved { .. } => TimelineTold::Moved { revision: Some(0), initializing: None },
        _ => TimelineTold::Reread,
    }
}

/// The request as the machine it lives on is asked it: unmarked.
pub fn unmarked(mut request: TimelineAsk) -> TimelineAsk {
    if let TimelineAsk::Moved { after, .. } | TimelineAsk::Changes { after, .. } | TimelineAsk::Files { after, .. } =
        &mut request
    {
        *after &= !ELSEWHERE;
    }
    request
}

/// What the machine it lives on told, its places marked.
pub fn marked(mut told: TimelineTold) -> TimelineTold {
    let mark = |place: &mut u64| *place |= ELSEWHERE;
    match &mut told {
        TimelineTold::Page { revision, events, through, .. } => {
            revision.iter_mut().for_each(mark);
            through.iter_mut().for_each(mark);
            events.iter_mut().for_each(|event| mark(&mut event.sequence));
        }
        TimelineTold::Changes { revision, changes, through, .. } => {
            revision.iter_mut().for_each(mark);
            mark(through);
            for change in changes {
                if let TimelineChange::Is { event } = change {
                    mark(&mut event.sequence);
                }
            }
        }
        TimelineTold::Moved { revision, .. } => revision.iter_mut().for_each(mark),
        TimelineTold::Files { through, .. } => mark(through),
        TimelineTold::Running { .. } | TimelineTold::Object { .. } | TimelineTold::Reread => {}
    }
    told
}

/// Whether `scope` is a conversation's copy held whole as one logged in
/// reads it: a conversation's or a body's conversation's scope, pulled
/// whole in its holding period, of which the hub lets this device hold a
/// class only those logged in hold. Whether one logged in here reads it now
/// is `read_by`'s to say.
pub fn conversation_copy(scope: &HeldScope) -> bool {
    let conversation = scope.key.starts_with("c/") || (scope.key.starts_with("b/") && !scope.key.ends_with("/log"));
    conversation
        && scope.pulled == Some(scope.holding)
        && scope.allowed.as_deref().is_some_and(|allowed| {
            allowed.lines().any(|class| {
                conversation_classes().iter().any(|logged| logged.holders == Holders::Logged && logged.prefix == class)
            })
        })
}

/// Who the hub last listed as reading a scope held here, as the driver
/// keeps it.
#[derive(serde::Deserialize)]
struct Listed {
    #[serde(default)]
    of: Vec<String>,
    #[serde(default)]
    frozen: bool,
}

/// Whether one of `logins` (of `only` alone where given) reads the held
/// `scope` now: one the hub last listed as reading it, its account in
/// `directory` as it was opened then and not locked, and reading it still
/// by the directory as it stands (AV01, AV02, AV09).
pub fn read_by(directory: &Directory, scope: &HeldScope, logins: &[AccountName], only: Option<&AccountName>) -> bool {
    let Some(descriptor) =
        scope.description.as_deref().and_then(|bytes| serde_json::from_slice::<Descriptor>(bytes).ok())
    else {
        return false;
    };
    let Some(listed) = scope.readers.as_deref().and_then(|text| serde_json::from_str::<Listed>(text).ok()) else {
        return false;
    };
    // A machine as the scope was described: its key and its owner as opened,
    // so another machine taking its name is not it (AW04).
    let as_described = |machine: &MachineName| {
        descriptor.born.get(&format!("machine:{machine}")).is_some_and(|was| {
            directory.members.iter().find(|member| member.name == *machine).is_some_and(|member| {
                let home = member.home.as_ref().map(|home| {
                    let opened = directory.account(home).map(|account| account.opened.as_millisecond().to_string());
                    format!("{home}@{}", opened.unwrap_or_default())
                });
                format!("{}:{}", member.key, home.unwrap_or_default()) == *was
            })
        })
    };
    let reads_now = |account: &AccountName| match &descriptor.readers {
        Readers::Conversation { openers, born_on } => fastctx_net::record_ownership::allows(
            directory,
            account,
            openers,
            born_on.as_ref().filter(|machine| as_described(machine)),
        ),
        Readers::Body { body, opener } => {
            account == opener || {
                let machine = directory
                    .members
                    .iter()
                    .find(|member| directory.body_of(&member.name).as_deref() == Some(body.as_str()));
                match machine {
                    // Its machine gone, its members frozen: as the hub listed
                    // them once it was gone, never as it listed them before
                    // (AW01).
                    None => listed.frozen,
                    Some(member) => {
                        member.home.as_ref() == Some(account)
                            || directory.shares.iter().any(|share| {
                                share.machine == member.name && share.active() && &share.account == account
                            })
                    }
                }
            }
        }
        // An account's records: the account itself.
        Readers::Account { account: whose } => account == whose,
        _ => false,
    };
    listed.of.iter().any(|reader| {
        let Some((name, opened)) = reader.rsplit_once('@') else { return false };
        let Ok(account) = AccountName::try_from(name.to_string()) else { return false };
        only.is_none_or(|only| *only == account)
            && logins.contains(&account)
            && directory
                .account(&account)
                .is_some_and(|found| !found.locked && found.opened.as_millisecond().to_string() == opened)
            && reads_now(&account)
    })
}

/// Whether the conversation of `scope` lives on `machine` as its copy here
/// has it, or was born there.
pub fn lives_on(library: &Library, scope: &HeldScope, machine: &MachineName) -> bool {
    if scope.key.starts_with(&format!("c/{}/{machine}/", scope.hub.as_deref().unwrap_or_default())) {
        return true;
    }
    library
        .item(&scope.key, fastctx_content::v2::ConversationMeta::STREAM, b"place")
        .ok()
        .flatten()
        .and_then(|entry| serde_json::from_slice::<serde_json::Value>(&entry.body).ok())
        .is_some_and(|place| place.get("machine").and_then(serde_json::Value::as_str) == Some(machine.as_str()))
}

/// The conversations `machine` keeps, as the copies among `scopes` in
/// `library` have them — copies a reader here reads, as the caller found
/// them — when the hub tells nothing of what is afoot there (plan 3-5,
/// design §28.32): each conversation's description, project, title, and
/// when it was archived; none of them is here or doing anything. None when
/// no copy says it lives there.
pub fn listed<'a>(
    library: &Library,
    scopes: impl IntoIterator<Item = &'a HeldScope>,
    machine: &MachineName,
) -> Option<WhoReply> {
    let mut residents = Vec::new();
    for scope in scopes {
        let Ok(items) = library.page(&scope.key, ConversationMeta::STREAM, None, 16) else { continue };
        let item = |name: &str| -> Option<serde_json::Value> {
            items
                .iter()
                .find(|entry| entry.item == name.as_bytes())
                .and_then(|entry| serde_json::from_slice(&entry.body).ok())
        };
        let (Some(mut about), Some(place)) = (item("about"), item("place")) else { continue };
        if place.get("machine").and_then(|lives| lives.as_str()) != Some(machine.as_str()) {
            continue;
        }
        // Where it lives now, as a list says of each conversation.
        about["machine"] = serde_json::Value::String(machine.to_string());
        let archived = item("archived").and_then(|archived| archived.get("at").cloned());
        let since = archived.clone().or_else(|| about.get("born_at").cloned()).unwrap_or(serde_json::Value::Null);
        let title = item("title").and_then(|title| title.get("title").cloned()).unwrap_or(serde_json::Value::Null);
        let now = serde_json::json!({
            "about": about,
            "town": place.get("project").cloned().unwrap_or(serde_json::Value::Null),
            "here": false,
            "now": { "activity": "idle", "since": since, "turn": null, "answering": [], "trouble": null, "title": title },
            "last_action": null,
            "removed": archived,
        });
        if let Ok(resident) = serde_json::from_value::<ResidentNow>(now) {
            residents.push(resident);
        }
    }
    (!residents.is_empty()).then(|| WhoReply {
        machine: machine.clone(),
        residents,
        jobs: Vec::new(),
        history: None,
        busy: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use fastctx_types::ResidentId;

    fn resident() -> ResidentId {
        ResidentId::try_from("claude-5a1ce004".to_string()).unwrap()
    }

    #[test]
    fn places_of_the_machine_it_lives_on_are_marked_and_unmarked_going_back() {
        let event = TimelineEvent {
            place: TimelinePlace { order: "01".into(), item: "02".into() },
            sequence: 7,
            likeness: None,
            shown: TimelineShown::Event { event: serde_json::json!({}) },
        };
        let told = TimelineTold::Page {
            revision: Some(5),
            initializing: None,
            events: vec![event],
            older: None,
            newer: None,
            through: Some(9),
        };
        let TimelineTold::Page { revision, events, through, .. } = marked(told) else { panic!("a page") };
        assert_eq!(revision, Some(5 | ELSEWHERE));
        assert_eq!(through, Some(9 | ELSEWHERE));
        assert_eq!(events[0].sequence, 7 | ELSEWHERE);
        let asked = TimelineAsk::Changes { resident: resident(), after: 9 | ELSEWHERE, max_events: 10, max_bytes: 10 };
        assert_eq!(after(&asked), Some(9 | ELSEWHERE));
        let TimelineAsk::Changes { after, .. } = unmarked(asked) else { panic!("changes") };
        assert_eq!(after, 9);
    }

    #[test]
    fn a_place_of_the_other_source_is_read_anew() {
        let moved = TimelineAsk::Moved { resident: resident(), after: 3 };
        assert!(matches!(anew(&moved), TimelineTold::Moved { revision: Some(0), .. }));
        let changes = TimelineAsk::Changes { resident: resident(), after: 3, max_events: 1, max_bytes: 1 };
        assert!(matches!(anew(&changes), TimelineTold::Reread));
        let page = TimelineAsk::Page { resident: resident(), from: TimelineFrom::Latest, max_events: 1, max_bytes: 1 };
        assert_eq!(after(&page), None);
    }

    #[test]
    fn places_go_as_lowercase_hexadecimal() {
        let place = Place { order: vec![0, 0xab], item: vec![0x10] };
        let told = encode_place(place.clone());
        assert_eq!((told.order.as_str(), told.item.as_str()), ("00ab", "10"));
        assert_eq!(decode_place(told).unwrap(), place);
        assert!(decode("ABCD").is_err());
        assert!(decode("abc").is_err());
        assert!(decode_place(TimelinePlace { order: "00".into(), item: String::new() }).is_err());
    }
}
