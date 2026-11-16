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
use fastctx_types::ipc::Failure;
use fastctx_types::{AccountName, ResidentId};

/// The mark of a place read from another machine.
pub(super) use fastctx_client::timeline_copy::{ELSEWHERE, after, anew, marked, unmarked};

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
pub(crate) use fastctx_client::timeline_copy::lives_on;

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
    read_here_as(node, scope, None)
}

/// `read_here`, as `only` reads it where given: the copy one account asks
/// for is one that account reads, not another logged in here.
pub(crate) fn read_here_as(node: &Node, scope: &fastctx_content::v2::HeldScope, only: Option<&AccountName>) -> bool {
    fastctx_client::timeline_copy::conversation_copy(scope) && reader_here_as(node, scope, only)
}

/// Whether the records' scope `scope` — an account's, or a body's — is read
/// by one logged in here, pulled whole in its holding period: what a
/// machine did and spent, read from here while it is away (plan 3-5).
pub(crate) fn records_here(node: &Node, scope: &fastctx_content::v2::HeldScope) -> bool {
    let account =
        scope.key.starts_with("a/") && scope.key.rsplit('/').next().is_some_and(|last| last.starts_with("records-"));
    let body = scope.key.starts_with("b/") && scope.key.ends_with("/records");
    (account || body) && scope.pulled == Some(scope.holding) && reader_here(node, scope)
}

/// The id of the hub named `hub` among those this machine is on.
pub(crate) fn hub_id_of(node: &Node, hub: &fastctx_types::HubName) -> Option<String> {
    node.links().into_iter().find(|link| link.hub_name() == *hub).map(|link| link.membership().id.to_string())
}

/// Whether the hub of the held scope `scope` is not reached now: no link to
/// it is online.
pub(crate) fn hub_away(node: &Node, scope: &fastctx_content::v2::HeldScope) -> bool {
    !node.links().iter().any(|link| {
        Some(link.membership().id.as_str()) == scope.hub.as_deref()
            && matches!(link.status(), fastctx_net::link::Status::Online { .. })
    })
}

/// An item's whole body as written: its body, or the one object it is in
/// when it was too large to be one (`{"in_object":true,…}`, AR03, M17).
pub(crate) use fastctx_client::records_copy::{object_bytes, whole_body};

/// Whether `machine` is away from every network this one shares with it:
/// offline on each that lists it.
pub(crate) fn away(node: &Node, machine: &fastctx_types::MachineName) -> bool {
    let mut listed = false;
    for link in node.links() {
        if let Some(member) = link.view().member(machine) {
            if member.online {
                return false;
            }
            listed = true;
        }
    }
    listed
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
    reader_here_as(node, scope, None)
}

/// `reader_here`, of `only` alone where given.
fn reader_here_as(node: &Node, scope: &fastctx_content::v2::HeldScope, only: Option<&AccountName>) -> bool {
    let Some(link) = node.links().into_iter().find(|link| Some(link.membership().id.as_str()) == scope.hub.as_deref())
    else {
        return false;
    };
    let logins: Vec<AccountName> = node.accounts_on(&link, &fastctx_types::access::Acting::Here).into_iter().collect();
    fastctx_client::timeline_copy::read_by(&link.view().directory, scope, &logins, only)
}

/// Where the copies here read by one logged in here hold the facts of the
/// job `job`: its conversation's scope (stream `jobs`), or, for a job run
/// for a conversation elsewhere or for a page, the records of the account
/// it ran for (stream `jobs/<machine>`, §28.58) — each with the stream and
/// the entry.
///
/// Each with what the job is named by there: its id in its conversation's
/// scope; in an account's records, which hold the jobs of many, its id and
/// a digest of what it ran for and when it began (`job_assets::key_of`).
pub(crate) fn job_facts(
    node: &Node,
    job: &fastctx_types::JobId,
) -> Vec<(fastctx_content::v2::HeldScope, String, String, fastctx_content::v2::Entry)> {
    let Some(assets) = node.assets.as_ref() else { return Vec::new() };
    let library = &assets.library;
    let Ok(scopes) = library.held_scopes() else { return Vec::new() };
    let mut found = Vec::new();
    for scope in scopes {
        if read_here(node, &scope) {
            let stream = fastctx_content::v2::JobFacts::STREAM.to_string();
            if let Ok(Some(entry)) = library.item(&scope.key, &stream, job.as_str().as_bytes()) {
                found.push((scope.clone(), stream, job.to_string(), entry));
            }
            continue;
        }
        if !records_here(node, &scope) {
            continue;
        }
        let prefix = format!("{}/", fastctx_content::v2::JobFacts::STREAM);
        let Ok(streams) = library.streams(&scope.key) else { continue };
        let keyed = format!("{job}~");
        for (stream, _) in streams.into_iter().filter(|(name, _)| name.starts_with(&prefix)) {
            let Ok(page) = library.page(&scope.key, &stream, Some((keyed.clone().into_bytes(), Vec::new())), 8) else {
                continue;
            };
            for entry in page.into_iter().take_while(|entry| entry.item.starts_with(keyed.as_bytes())) {
                let Ok(named) = String::from_utf8(entry.item.clone()) else { continue };
                found.push((scope.clone(), stream.clone(), named, entry));
            }
        }
    }
    found
}

/// The record of the job `job`, as a copy here read by one logged in here
/// holds it: what is known of a command run for a conversation whose
/// machine is away (§28.33). Job ids are unique on their machine alone:
/// two copies holding one of that id, and none is told.
pub(crate) fn job(node: &Node, job: &fastctx_types::JobId) -> Option<fastctx_types::run::JobRecord> {
    let library = &node.assets.as_ref()?.library;
    let [(_, _, _, entry)] = job_facts(node, job).try_into().ok()?;
    let mut value = serde_json::from_slice::<serde_json::Value>(&entry.body).ok()?;
    // A script too large for the record is its one object (§28.58); one
    // of before said only how long it was (QQ12).
    if let Some(said) = value.get("script").filter(|script| script.is_object()).cloned() {
        let whole = (said.get("in_object").and_then(serde_json::Value::as_bool) == Some(true))
            .then(|| entry.objects.first().and_then(|object| object_bytes(library, object)))
            .flatten()
            .and_then(|bytes| String::from_utf8(bytes).ok());
        let bytes = said.get("bytes").and_then(serde_json::Value::as_u64).unwrap_or(0);
        value["script"] = serde_json::Value::String(
            whole.unwrap_or_else(|| format!("(a script of {bytes} bytes, not kept with the record)")),
        );
    }
    serde_json::from_value::<fastctx_types::run::JobRecord>(value).ok()
}
