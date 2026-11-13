//! A conversation of another machine read from this machine's copy of it
//! (plan 3-5, design §28.30). Once the copy is pulled whole in the holding
//! period it is in, and someone logged in here reads the conversation, a
//! reader reads the copy and asks the machine it lives on nothing: while
//! that machine is away, its conversations are read all the same. Reads
//! only read: nothing is fetched for them.
//!
//! A place read from another machine (a page's or changes' revision and
//! count, an event's sequence) carries [`ELSEWHERE`]; one read here never
//! does, its era having 24 bits. A place of one source asked of the other
//! is read anew, never taken for a number of the other's library.

use super::local::{self, Held};
use crate::Node;
use fastctx_content::v2::{Kind as _, Timeline};
use fastctx_types::access::Written;
use fastctx_types::assets::{Holders, Readers, conversation_classes};
use fastctx_types::{AccountName, ResidentId};
use fastctx_types::ipc::Failure;
use fastctx_types::timeline_pages::{TimelineAsk, TimelineChange, TimelineTold};

/// The mark of a place read from another machine.
pub(super) const ELSEWHERE: u64 = 1 << 63;

fn failure(error: impl std::fmt::Display) -> Failure {
    Failure::new(error.to_string())
}

/// This machine's copy of the conversation, when a reader here reads it:
/// held whole in its holding period, held as one logged in here reads it
/// (the hub lets it hold a class only those logged in hold), and its
/// timeline begun. One held only because this machine works for one of
/// its readers is read by no one here (§20, 3-3).
pub(crate) fn of(node: &Node, resident: &ResidentId, machine: Option<&Written>) -> Result<Option<Held>, Failure> {
    let Some(kept) = node.assets.as_ref() else { return Ok(None) };
    let library = kept.library.clone();
    let mut found = None;
    for scope in library.scopes_ending(&format!("/{resident}")).map_err(failure)? {
        if !read_here(node, &scope) {
            continue;
        }
        // Asked of a machine, the copy of the conversation that lives there
        // or was born there, never another machine's of the same id (AU04),
        // nor one of a machine of that name on another network (AV03).
        if let Some(machine) = machine
            && !(lives_on(&library, &scope, &machine.name) && of_network(node, &scope, machine))
        {
            continue;
        }
        let Some(at) = library.view_revision(&scope.key, Timeline::STREAM).map_err(failure)? else { continue };
        if at & ((1 << 40) - 1) == 0 {
            continue;
        }
        // Ids are unique on their machine alone: two copies of that id, and
        // the one asked of is read where it lives.
        if found.replace(scope.key).is_some() {
            return Ok(None);
        }
    }
    Ok(found.map(|scope| local::Held::at(library, scope)))
}

/// Whether the conversation of `scope` lives on `machine` as its copy here
/// has it, or was born there.
fn lives_on(library: &fastctx_content::v2::Library, scope: &fastctx_content::v2::HeldScope, machine: &fastctx_types::MachineName) -> bool {
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

/// Whether `scope` is of the network `machine` is named on: a name given with
/// its hub's is that hub's machine alone; one without names the machine of
/// the one network here that has a machine of that name. Two networks having
/// one, the name is not this copy's to settle: the machine is asked, and
/// says it is ambiguous (RR01). None having one — the machine gone — its copy
/// is what there is.
fn of_network(node: &Node, scope: &fastctx_content::v2::HeldScope, machine: &Written) -> bool {
    let links = node.links();
    if let Some(hub) = &machine.hub {
        return links
            .into_iter()
            .find(|link| link.hub_name() == *hub)
            .is_some_and(|link| scope.hub.as_deref() == Some(link.membership().id.as_str()));
    }
    let naming: Vec<_> = links.iter().filter(|link| link.view().member(&machine.name).is_some()).collect();
    match naming.as_slice() {
        [] => true,
        [link] => scope.hub.as_deref() == Some(link.membership().id.as_str()),
        _ => false,
    }
}

/// Whether `scope` is a conversation's copy held whole here as one logged
/// in here reads it: a conversation's or a body's conversation's scope,
/// pulled whole in its holding period, of which the hub lets this machine
/// hold a class only those logged in hold.
pub(crate) fn read_here(node: &Node, scope: &fastctx_content::v2::HeldScope) -> bool {
    let conversation = scope.key.starts_with("c/") || (scope.key.starts_with("b/") && !scope.key.ends_with("/log"));
    conversation
        && scope.pulled == Some(scope.holding)
        && scope.allowed.as_deref().is_some_and(|allowed| {
            allowed.lines().any(|class| {
                conversation_classes().iter().any(|logged| logged.holders == Holders::Logged && logged.prefix == class)
            })
        })
        && reader_here(node, scope)
}

/// Whether one the hub last said reads the scope is logged in here now as
/// the same account, unlocked: the hub's own gate judged who reads it — a
/// body's frozen members, an opener as the account it was, the holders of
/// the machine it was born on — and what it let this machine hold is no
/// leave for one logged out, locked or made again since (AU03, AV01, AV02,
/// AV09). Changes the hub's next listing has not judged yet are judged here
/// too: an opener or holder of the machine it was born on as the directory
/// has them now, a body's members as they are while its machine is there.
fn reader_here(node: &Node, scope: &fastctx_content::v2::HeldScope) -> bool {
    let Some(link) = node.links().into_iter().find(|link| Some(link.membership().id.as_str()) == scope.hub.as_deref()) else {
        return false;
    };
    let Some(descriptor) =
        scope.description.as_deref().and_then(|bytes| serde_json::from_slice::<fastctx_types::assets::Descriptor>(bytes).ok())
    else {
        return false;
    };
    let Some(listed) = scope.readers.as_deref().and_then(|text| serde_json::from_str::<Listed>(text).ok()) else {
        return false;
    };
    let logins = node.accounts_on(&link, &fastctx_types::access::Acting::Here);
    let view = link.view();
    let directory = &view.directory;
    // A machine as the scope was described: its key and its owner as opened,
    // so another machine taking its name is not it (AW04).
    let as_described = |machine: &fastctx_types::MachineName| {
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
        Readers::Conversation { openers, born_on } => {
            fastctx_net::record_ownership::allows(directory, account, openers, born_on.as_ref().filter(|machine| as_described(machine)))
        }
        Readers::Body { body, opener } => {
            account == opener || {
                let machine =
                    directory.members.iter().find(|member| directory.body_of(&member.name).as_deref() == Some(body.as_str()));
                match machine {
                    // Its machine gone, its members frozen: as the hub listed
                    // them once it was gone, never as it listed them before
                    // (AW01).
                    None => listed.frozen,
                    Some(member) => {
                        member.home.as_ref() == Some(account)
                            || directory.shares.iter().any(|share| share.machine == member.name && share.active() && &share.account == account)
                    }
                }
            }
        }
        _ => false,
    };
    listed.of.iter().any(|reader| {
        let Some((name, opened)) = reader.rsplit_once('@') else { return false };
        let Ok(account) = AccountName::try_from(name.to_string()) else { return false };
        logins.contains(&account)
            && directory
                .account(&account)
                .is_some_and(|found| !found.locked && found.opened.as_millisecond().to_string() == opened)
            && reads_now(&account)
    })
}

/// Who the hub last listed as reading a scope held here (`Held.readers`,
/// `Held.frozen`), as `assets::session` keeps it.
#[derive(serde::Deserialize)]
struct Listed {
    #[serde(default)]
    of: Vec<String>,
    #[serde(default)]
    frozen: bool,
}

/// The record of the job `job`, as a copy here read by one logged in here
/// holds it: what is known of a command run for a conversation whose
/// machine is away (§28.33). Job ids are unique on their machine alone:
/// two copies holding one of that id, and none is told.
pub(crate) fn job(node: &Node, job: &fastctx_types::JobId) -> Option<fastctx_types::run::JobRecord> {
    let library = &node.assets.as_ref()?.library;
    let mut found = None;
    for scope in library.held_scopes().ok()? {
        if !read_here(node, &scope) {
            continue;
        }
        let Ok(Some(entry)) = library.item(&scope.key, fastctx_content::v2::JobFacts::STREAM, job.as_str().as_bytes()) else {
            continue;
        };
        // A script too large for the record is said, not given (QQ12).
        let Ok(mut value) = serde_json::from_slice::<serde_json::Value>(&entry.body) else { continue };
        if let Some(omitted) = value.get("script").filter(|script| script.is_object()).cloned() {
            let bytes = omitted.get("bytes").and_then(serde_json::Value::as_u64).unwrap_or(0);
            value["script"] = serde_json::Value::String(format!("(a script of {bytes} bytes, not kept with the record)"));
        }
        let Ok(record) = serde_json::from_value::<fastctx_types::run::JobRecord>(value) else { continue };
        if found.replace(record).is_some() {
            return None;
        }
    }
    found
}

/// Where a reader asks to read on from, when it does.
pub(super) fn after(request: &TimelineAsk) -> Option<u64> {
    match request {
        TimelineAsk::Moved { after, .. } | TimelineAsk::Changes { after, .. } => Some(*after),
        _ => None,
    }
}

/// The answer to a reader whose place is of another source: read anew.
pub(super) fn anew(request: &TimelineAsk) -> TimelineTold {
    match request {
        TimelineAsk::Moved { .. } => TimelineTold::Moved { revision: Some(0), initializing: None },
        _ => TimelineTold::Reread,
    }
}

/// The request as the machine it lives on is asked it: unmarked.
pub(super) fn unmarked(mut request: TimelineAsk) -> TimelineAsk {
    if let TimelineAsk::Moved { after, .. } | TimelineAsk::Changes { after, .. } = &mut request {
        *after &= !ELSEWHERE;
    }
    request
}

/// What the machine it lives on told, its places marked.
pub(super) fn marked(mut told: TimelineTold) -> TimelineTold {
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
        TimelineTold::Running { .. } | TimelineTold::Object { .. } | TimelineTold::Reread => {}
    }
    told
}
