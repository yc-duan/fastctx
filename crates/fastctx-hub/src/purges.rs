//! Deleting what was born under an assistance its giver ended for good
//! (design-forget-account-v2.md F1/F3 v2). Only an ending that says so
//! deletes anything: the giver's own ending, naming the very assistance it
//! ends; one that ran out, by the hub; an account removed. Any other ending,
//! as of a login's other spelling replaced or of an earlier client, only
//! ends it. The purge is kept with the directory, written in the very
//! commit that ends the assistance, with who it was for and the machine
//! that gave it frozen as they were.
//!
//! It goes in steps, each asked of a machine by its key until it answers
//! the round it was asked in: the machine that gave it closes it, for
//! good, and tells what was born there under it; the hub decides, of each
//! conversation, whether it goes whole; each goes where it is now, and only
//! then what the hub keeps of it.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use fastctx_net::sign::PublicKey;
use fastctx_types::access::GrantId;
use fastctx_types::{AccountName, AgentName, MachineName, ResidentId};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

/// The purges the hub keeps, with the directory (`crate::store`).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct Purges {
    /// The number the last purge took.
    #[serde(default)]
    pub(crate) last: u64,
    #[serde(default)]
    pub(crate) kept: Vec<Arc<Purge>>,
}

/// One assistance ended for good, and what is deleted of what was born
/// under it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Purge {
    pub(crate) id: u64,
    /// Every grant the assistance was (`Assist::grants`).
    pub(crate) grants: Vec<GrantId>,
    /// The account it assisted, and when that account of its name was
    /// opened: one opened again under the name is another.
    pub(crate) account: AccountName,
    pub(crate) generation: Timestamp,
    /// The machine whose login gave it, by its key: a machine renamed is
    /// the same machine, and one that took the name later is not.
    pub(crate) machine: MachineName,
    pub(crate) key: PublicKey,
    pub(crate) agent: AgentName,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) home: Option<PathBuf>,
    pub(crate) at: Timestamp,
    /// The account that ended it; none where the hub did (its deadline).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) by: Option<AccountName>,
    /// Which asking of its current step this is.
    #[serde(default)]
    pub(crate) round: u64,
    pub(crate) step: Step,
    /// Each conversation found born under it, and what became of it.
    #[serde(default)]
    pub(crate) items: BTreeMap<ResidentId, Item>,
    /// What the machine asked last said it could not do yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) waiting: Option<String>,
    /// The machines that may keep copies of what goes, as they were as it
    /// was ended: those logged in to the account then, which its removal
    /// takes away (codex-d515b194). By their keys: one renamed since is told
    /// under its name now, and one that took the name of one gone is not
    /// told (codex-d515b194).
    #[serde(default, rename = "audience_keys", skip_serializing_if = "BTreeSet::is_empty")]
    pub(crate) audience: BTreeSet<PublicKey>,
    /// As builds before kept them: by name, or by key, under the earlier
    /// field's name; read as either (codex-d515b194, codex-87c8bbd9).
    #[serde(default, rename = "audience", skip_serializing_if = "BTreeSet::is_empty")]
    pub(crate) audience_named: BTreeSet<String>,
}

/// Where a purge stands.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "step")]
pub(crate) enum Step {
    /// The machine that gave it is to close it and tell what was born there
    /// under it.
    Closing,
    /// Each conversation decided to go whole is deleted where it is now,
    /// then what the hub keeps of it.
    Deleting,
    /// Nothing more is to be done; what was kept, or could not be reached,
    /// is said in its items.
    Done,
}

/// A conversation found born under the assistance purged.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Item {
    /// The machine it lives on now, by its key, where the hub knows it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) host: Option<PublicKey>,
    /// The machines it lived on before, whose scopes of it go too.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) previous: Vec<MachineName>,
    pub(crate) fate: Fate,
    /// Which conversation of its id it is, by where and when it was born
    /// (`owned_records::Conversation::identity`), where known: kept to be
    /// looked at; `deleting` goes by the id alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) identity: Option<Box<fastctx_types::sync::identity::ObjectId>>,
    /// What of it the hub found and did not let go of, not proven its own:
    /// said for a person to look at (codex-d515b194).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) left: Option<String>,
}

/// What becomes of a conversation found born under an assistance purged.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "fate")]
pub(crate) enum Fate {
    /// It goes whole: where it is now first.
    Going,
    /// It went where it was; what the hub keeps of it goes now.
    GoneThere,
    /// Nothing of it is kept any more.
    Gone,
    /// It is kept, and why: a colleague's, working for another account too,
    /// a body's; or part of it, as an original a fork needs. Said for a
    /// person to look at.
    Kept { why: String },
    /// The machine it is on left the hub before it went there: what the hub
    /// kept of it went; what is there, the hub cannot reach.
    Unreachable,
}

impl Purge {
    /// Whether it still has something to ask of a machine.
    pub(crate) fn open(&self) -> bool {
        self.step != Step::Done
    }
}

/// Whether the conversation `resident` that `account`, as `opened`, opened
/// was deleted for a purge, or is being: decided to go, gone, or out of the
/// hub's reach. It moves no more, and is not registered anew, wherever it
/// comes from, however late: the hub let go of what it keeps of it for good
/// (codex-87c8bbd9, a copy taken elsewhere before it went, registered after
/// its purge was done). One kept stays as it was. By its id alone, as the
/// account's records keep every conversation by its id alone: what a host
/// says of where and when it was born is not proof it is another
/// (codex-d515b194), and two machines' ids meet as rarely here as anywhere
/// in those records.
pub(crate) fn deleting(purges: &Purges, account: &AccountName, opened: Timestamp, resident: &str) -> bool {
    purges.kept.iter().any(|purge| {
        purge.account == *account
            && purge.generation == opened
            && purge.items.iter().any(|(id, item)| id.as_str() == resident && !matches!(item.fate, Fate::Kept { .. }))
    })
}

/// The grants purged, done or not: a conversation that says it was born
/// under one is registered no more.
pub(crate) fn closed(purges: &Purges) -> BTreeSet<&GrantId> {
    purges.kept.iter().flat_map(|purge| &purge.grants).collect()
}

/// An assistance ended for good, as the change that ends it says.
pub(crate) struct Ended {
    pub(crate) grants: Vec<GrantId>,
    pub(crate) machine: MachineName,
    pub(crate) account: AccountName,
    pub(crate) agent: AgentName,
    pub(crate) home: Option<PathBuf>,
    pub(crate) by: Option<AccountName>,
    /// The machines that may keep copies of it, frozen as it ends
    /// (`Purge::audience`); none where the account stays, as those logged in
    /// to it are found as its conversations go.
    pub(crate) audience: BTreeSet<PublicKey>,
}

/// Keeps the purge of the assistance `ended`, in the commit that ends it:
/// the book is written whole with it (`BookEdit::Full`), the change's own
/// rows and the purge's at once. An assistance purged already is not purged
/// again.
pub(crate) fn begin(store: &mut crate::store::Store, ended: Ended) -> Result<(), String> {
    if ended.grants.is_empty() {
        return Err("the assistance has no grant on the hub to end for good".into());
    }
    let directory = &store.book.directory;
    let generation =
        directory.account(&ended.account).ok_or_else(|| format!("there is no account {}", ended.account))?.opened;
    let member =
        directory.member(&ended.machine).ok_or_else(|| format!("there is no machine named {}", ended.machine))?;
    let (machine, key) = (member.name.clone(), member.key);
    if store.book.purges.kept.iter().any(|purge| purge.grants.iter().any(|grant| ended.grants.contains(grant))) {
        return Ok(());
    }
    store.book.purges.last += 1;
    let purge = Purge {
        id: store.book.purges.last,
        grants: ended.grants,
        account: ended.account,
        generation,
        machine,
        key,
        agent: ended.agent,
        home: ended.home,
        at: Timestamp::now(),
        by: ended.by,
        round: 1,
        step: Step::Closing,
        items: BTreeMap::new(),
        waiting: None,
        audience: ended.audience,
        audience_named: BTreeSet::new(),
    };
    store.book.purges.kept.push(Arc::new(purge));
    store.book_edit = crate::store::BookEdit::Full;
    Ok(())
}

/// Whether this hub may hand itself over to `to`: once it purges, only to a
/// machine whose session says it does too, so no purge kept is left to a
/// hub that would not carry it on (codex-87c8bbd9).
pub(crate) fn takes_over(hub: &crate::Hub, to: &MachineName) -> Result<(), String> {
    // As its last session said, by the key it has now: the hub's sessions
    // are closed once it is frozen to hand over (codex-87c8bbd9).
    let store = hub.store();
    let key = store.book.directory.member(to).map(|member| member.key);
    if key.is_some() && store.letters.get(to).and_then(|letters| letters.purges) == key {
        return Ok(());
    }
    Err(format!(
        "update {to} before moving the hub there: this hub deletes what was born under an assistance ended for good, and its version does not"
    ))
}

/// What a purge asks of which machine now, by the machine's key.
fn asked(purge: &Purge) -> Vec<(PublicKey, fastctx_net::wire::Purging)> {
    let ask = |step| fastctx_net::wire::Purging {
        id: purge.id,
        round: purge.round,
        grants: purge.grants.clone(),
        account: purge.account.clone(),
        generation: purge.generation,
        step,
    };
    match purge.step {
        Step::Closing => vec![(purge.key, ask(fastctx_net::wire::PurgeStep::Close))],
        Step::Deleting => {
            let mut by_host: BTreeMap<PublicKey, Vec<ResidentId>> = BTreeMap::new();
            for (resident, item) in &purge.items {
                if item.fate == Fate::Going
                    && let Some(host) = item.host
                {
                    by_host.entry(host).or_default().push(resident.clone());
                }
            }
            by_host
                .into_iter()
                .map(|(host, residents)| (host, ask(fastctx_net::wire::PurgeStep::Delete { residents })))
                .collect()
        }
        Step::Done => Vec::new(),
    }
}

/// Asks each machine connected now what its purges ask of it, where its
/// session says it purges; of the machine `member` only, when given. A
/// machine of an earlier release is asked nothing: its part waits for it.
pub(crate) fn tell(hub: &crate::Hub, member: Option<&MachineName>) {
    tell_of(hub, member, None);
}

/// `tell`, of the purge `id` only, when given.
fn tell_of(hub: &crate::Hub, member: Option<&MachineName>, id: Option<u64>) {
    // Nothing is asked while the hub is frozen to hand itself over.
    if hub.moving.frozen() {
        return;
    }
    let tell: Vec<(MachineName, PublicKey, fastctx_net::wire::Purging)> = {
        let store = hub.store();
        let directory = &store.book.directory;
        store
            .book
            .purges
            .kept
            .iter()
            .filter(|purge| purge.open() && id.is_none_or(|id| purge.id == id))
            .flat_map(|purge| asked(purge))
            .filter_map(|(key, purging)| {
                let to = directory.members.iter().find(|found| found.key == key)?;
                member.is_none_or(|member| directory.same(&to.name, member)).then(|| (to.name.clone(), key, purging))
            })
            .collect()
    };
    if tell.is_empty() {
        return;
    }
    let sessions = hub.sessions();
    for (member, key, purging) in tell {
        // The session of the very device asked, not one that took its name
        // since (codex-d515b194).
        if let Some(session) = sessions.get(&member).filter(|session| session.purges && session.key == key) {
            let _ = session.out.send(crate::Out::Frame(fastctx_net::wire::Down::Purge { purging }));
        }
    }
}

/// What the machine `member` did with the step of the purge `id` asked in
/// `round`. An answer to an earlier round, or from a machine not asked, is
/// not taken.
/// `key` is the one its session proved: an answer is taken by it, not by
/// the name, which another device may have taken since (codex-d515b194).
pub(crate) fn purged(
    hub: &crate::Hub,
    member: &MachineName,
    key: PublicKey,
    id: u64,
    round: u64,
    outcome: fastctx_net::wire::Purged,
) {
    use fastctx_net::wire::{PurgeResult, Purged};
    // Not taken while the hub is frozen to hand itself over: asked again by
    // the hub that takes over. Counted until written, so freezing waits.
    let Some(arc) = hub.this.get().and_then(std::sync::Weak::upgrade) else { return };
    let Some(_entered) = crate::moving::Moving::enter(&arc) else { return };
    let hub_id = hub.store().hub.id.clone();
    let (went, advanced) = {
        let mut store = hub.edit_store();
        let Some(index) = store.book.purges.kept.iter().position(|purge| purge.id == id) else { return };
        let kept = &store.book.purges.kept[index];
        if kept.round != round || !asked(kept).iter().any(|(asked, _)| *asked == key) {
            return;
        }
        let old = kept.clone();
        let mut purge = Purge::clone(kept);
        let mut went = Vec::new();
        match (&purge.step, outcome) {
            (Step::Closing, Purged::Closed { born }) => {
                let owners = hub.account_records.owners.lock().expect("never poisoned");
                purge.items = decide(&hub_id, &store.book.directory, &owners, &purge, born);
                drop(owners);
                purge.step =
                    if purge.items.values().any(|item| item.fate == Fate::Going) { Step::Deleting } else { Step::Done };
                purge.round += 1;
                purge.waiting = None;
                hub.log.line(format_args!(
                    "{member} closed the assistance of {} purged ({}): {} conversations born under it, {} to go",
                    purge.account,
                    purge.id,
                    purge.items.len(),
                    purge.items.values().filter(|item| item.fate == Fate::Going).count()
                ));
            }
            (Step::Deleting, Purged::Deleted { results }) => {
                for (resident, result) in results {
                    let Some(item) = purge.items.get_mut(&resident) else { continue };
                    if item.fate != Fate::Going || item.host != Some(key) {
                        continue;
                    }
                    match result {
                        PurgeResult::Deleted => {
                            item.fate = Fate::GoneThere;
                            went.push(resident);
                        }
                        PurgeResult::Kept { why } => item.fate = Fate::Kept { why },
                        PurgeResult::NotBornUnder => {
                            item.fate =
                                Fate::Kept { why: "its machine says it was not born under the assistance".into() }
                        }
                        PurgeResult::NotHere => {
                            item.fate =
                                Fate::Kept { why: "its machine no longer has it, nor deleted it for this".into() }
                        }
                        // Not done there yet: asked again.
                        PurgeResult::Finishing { why } | PurgeResult::Busy { why } | PurgeResult::Failed { why } => {
                            purge.waiting = Some(format!("{resident}: {why}"));
                        }
                    }
                }
                purge.round += 1;
                if !purge.items.values().any(|item| matches!(item.fate, Fate::Going | Fate::GoneThere)) {
                    purge.step = Step::Done;
                }
            }
            (_, Purged::Waiting { why }) => purge.waiting = Some(why),
            (_, Purged::Failed { reason }) => purge.waiting = Some(reason),
            (step, other) => {
                hub.log.line(format_args!("{member} answered the purge {id} at {step:?} with {other:?}: not taken"));
                return;
            }
        }
        // A step taken is asked at once; what is not done yet waits to be
        // asked again (`AGAIN`), so a machine that keeps failing is not
        // asked over and over (codex-87c8bbd9).
        let advanced = purge.step != old.step;
        store.book.purges.kept[index] = Arc::new(purge);
        store.book_edit = crate::store::BookEdit::Full;
        if let Err(error) = store.save_book() {
            // Not kept: it stands as it did, and is asked again as it did.
            store.book.purges.kept[index] = old;
            hub.log.line(format_args!("keeping what {member} did with the purge {id}: {error}"));
            return;
        }
        (went, advanced)
    };
    for resident in &went {
        hub.log.line(format_args!("the purge {id}: {resident} went where it lived"));
    }
    settle(hub);
    if advanced {
        tell_of(hub, None, Some(id));
    }
}

/// Decides, of each conversation found born under the purge's assistance,
/// whether it goes whole: only one its machine found born there under it,
/// for that account alone, no colleague's, no body's, never moved, its
/// record, where the hub keeps one, saying the same (codex-87c8bbd9). Every
/// other is kept, and said, for a person to look at.
fn decide(
    hub: &fastctx_types::access::HubId,
    directory: &fastctx_net::directory::Directory,
    owners: &crate::owned_records::index::Index,
    purge: &Purge,
    born: Vec<fastctx_net::wire::BornUnder>,
) -> BTreeMap<ResidentId, Item> {
    let kept = |why: &str| Fate::Kept { why: why.to_string() };
    let mut items = BTreeMap::new();
    for found in born {
        let record = owners.opened(&purge.account, purge.generation, found.resident.as_str());
        let fate = if found.colleague {
            kept("a colleague's conversation")
        } else if found.others {
            kept("it works for another account as well")
        } else {
            match record {
                Some((_, true)) => kept("a body's conversation"),
                Some((conversation, _)) if conversation.birth.colleague => kept("a colleague's conversation"),
                Some((conversation, _))
                    if !crate::owned_records::deletion::never_moved(directory, conversation, &purge.key) =>
                {
                    kept("it moved since it was born")
                }
                Some((conversation, _))
                    if conversation.birth.generation != Some(purge.generation)
                        || ![&conversation.birth.born_under, &conversation.birth.claimed_under]
                            .into_iter()
                            .flatten()
                            .any(|grant| purge.grants.contains(grant)) =>
                {
                    kept("its record says it was born otherwise")
                }
                _ => Fate::Going,
            }
        };
        // Which conversation of its id it is: as its record says, or as its
        // machine says it was born there (codex-87c8bbd9).
        let identity = record.and_then(|(conversation, _)| conversation.identity.clone()).or_else(|| {
            found.born_at.map(|born_at| {
                Box::new(crate::owned_records::conversation_identity(hub, &purge.key, &found.resident, born_at))
            })
        });
        items.insert(found.resident, Item { host: Some(purge.key), previous: Vec::new(), fate, identity, left: None });
    }
    // Registered as born under it, and not found born there: where it is
    // now, or why its machine did not find it, is for a person to look at.
    for conversation in owners.born_under(&purge.account, purge.generation, &purge.grants) {
        let resident = &conversation.address.resident;
        if items.contains_key(resident) {
            continue;
        }
        let host = directory.member(&conversation.address.machine).map(|member| member.key);
        let why = format!(
            "its record says it was born under the assistance, and its machine did not find it so; it is on {} now",
            conversation.address.machine
        );
        items.insert(
            resident.clone(),
            Item {
                host,
                previous: conversation.previous.clone(),
                fate: kept(&why),
                identity: conversation.identity.clone(),
                left: None,
            },
        );
    }
    items
}

/// Lets go of what the hub keeps of each conversation gone where it lived:
/// its scopes and the copies other members keep, as for a deletion its
/// machine asked for (`forget::gone_there`); its record, plan and history, as
/// the hub itself (`owned_records::deletion::erase`), as its machine may no
/// longer act for the account. Then it is gone; one whose record could not
/// be let go of now is tried again as purges are looked at again.
pub(crate) fn settle(hub: &crate::Hub) {
    static SETTLING: std::sync::LazyLock<Mutex<BTreeSet<(u64, ResidentId)>>> =
        std::sync::LazyLock::new(Default::default);
    let Some(hub) = hub.this.get().and_then(std::sync::Weak::upgrade) else { return };
    // Counted as the records may change from here, before anything is read:
    // a hub freezing to hand itself over waits for it, or it does nothing
    // (codex-87c8bbd9).
    let Some(_entered) = crate::moving::Moving::enter(&hub) else { return };
    let went: Vec<(Arc<Purge>, ResidentId)> = {
        let store = hub.store();
        store
            .book
            .purges
            .kept
            .iter()
            .flat_map(|purge| {
                purge
                    .items
                    .iter()
                    .filter(|(_, item)| item.fate == Fate::GoneThere)
                    .map(|(resident, _)| (purge.clone(), resident.clone()))
            })
            .collect()
    };
    for (purge, resident) in went {
        let id = purge.id;
        if !SETTLING.lock().expect("never poisoned").insert((id, resident.clone())) {
            continue;
        }
        let hub = hub.clone();
        // It changes the records: a hub moving waits for it (`moving`).
        let Some(held) = crate::moving::Moving::enter(&hub) else {
            SETTLING.lock().expect("never poisoned").remove(&(id, resident));
            return;
        };
        fastctx_platform::task::spawn(async move {
            let _held = held;
            let by = (purge.machine.clone(), purge.key);
            let settled = settle_one(&hub, &purge, &resident, by).await;
            if let Err(error) = settled {
                hub.log.line(format_args!("letting go of what the hub keeps of {resident}, purged ({id}): {error}"));
            }
            SETTLING.lock().expect("never poisoned").remove(&(id, resident));
        });
    }
}

/// `settle` of one conversation gone where it lived; an error where it is
/// to be tried again.
async fn settle_one(
    hub: &Arc<crate::Hub>,
    purge: &Purge,
    resident: &ResidentId,
    by: (MachineName, PublicKey),
) -> Result<(), String> {
    let (id, account, generation, grants) = (purge.id, &purge.account, purge.generation, &purge.grants[..]);
    let address = fastctx_types::Address { machine: by.0.clone(), resident: resident.clone() };
    use crate::owned_records::deletion::Erased;
    // Whose its record is, before anything of it on the hub is let go of: a
    // conversation moved since keeps all it has here (codex-87c8bbd9).
    if crate::owned_records::deletion::judge(hub, by.clone(), account, generation, resident, grants).await?
        == Erased::Another
    {
        let resident = resident.clone();
        let why = "its record on the hub is another conversation's of the same id, or it moved";
        hub.blocking(move |hub| kept_on_hub(&hub, id, &resident, why)).await;
        return Ok(());
    }
    // Its scopes: its own deleted for good, described or not, so one on its
    // way is refused after, and every other it has of that account as
    // opened. Its key held by another conversation of the same id, nothing
    // of it is let go of on the hub: kept, for a person to look at
    // (codex-87c8bbd9).
    let (scopes, opener, key) = (address.clone(), account.clone(), by.1);
    let own =
        hub.blocking(move |hub| crate::assets::deleted::purge_scopes(&hub, &scopes, key, &opener, generation)).await?;
    if !own {
        let resident = resident.clone();
        let why = "its scope on the hub is another conversation's of the same id";
        hub.blocking(move |hub| kept_on_hub(&hub, id, &resident, why)).await;
        return Ok(());
    }
    // As recorded before its record goes: who else keeps copies of it.
    let ownership = hub
        .account_records
        .owners
        .lock()
        .expect("never poisoned")
        .opened(account, generation, resident.as_str())
        .map(|(conversation, _)| conversation.clone());
    // Its record, each version let go of only where it is this very
    // conversation's (codex-87c8bbd9).
    let (erased, left) = crate::owned_records::deletion::erase(hub, by, account, generation, resident, grants).await?;
    if erased == Erased::Another {
        let resident = resident.clone();
        let why = "its record on the hub is another conversation's of the same id, or it moved";
        hub.blocking(move |hub| kept_on_hub(&hub, id, &resident, why)).await;
        return Ok(());
    }
    // The copies others keep of it, by its address; its scopes went above.
    let ownership = ownership.filter(|_| erased == Erased::Gone);
    let forgetting = account.clone();
    let mut audience = purge.audience.clone();
    // A name kept then is not proof which device it was: it is not told,
    // and said (codex-d515b194).
    let mut unproven = Vec::new();
    for said in &purge.audience_named {
        match serde_json::from_value::<PublicKey>(serde_json::Value::String(said.clone())) {
            Ok(key) => {
                audience.insert(key);
            }
            Err(_) => unproven.push(said.as_str()),
        }
    }
    let left = match (left, unproven.is_empty()) {
        (left, true) => left,
        (left, false) => Some(
            left.into_iter()
                .chain([format!("copies on machines kept by name only, not told: {}", unproven.join(", "))])
                .collect::<Vec<_>>()
                .join("; "),
        ),
    };
    hub.blocking(move |hub| crate::forget::gone_there(&hub, address, forgetting, ownership, true, audience)).await?;
    let resident = resident.clone();
    hub.blocking(move |hub| gone(&hub, id, &resident, left)).await;
    Ok(())
}

/// What the hub keeps of `resident`, of the purge `id`, is kept, and why.
fn kept_on_hub(hub: &crate::Hub, id: u64, resident: &ResidentId, why: &str) {
    hub.log.line(format_args!("the purge {id}: what the hub keeps of {resident} stays: {why}"));
    settled_as(hub, id, resident, Fate::Kept { why: why.to_string() }, None);
}

/// Nothing is kept any more of `resident`, of the purge `id`, but what is
/// `left`, not proven its own.
fn gone(hub: &crate::Hub, id: u64, resident: &ResidentId, left: Option<String>) {
    if let Some(left) = &left {
        hub.log.line(format_args!("the purge {id}: of {resident}, the hub left what is not proven its own: {left}"));
    }
    settled_as(hub, id, resident, Fate::Gone, left);
}

/// `resident`, of the purge `id`, comes to `fate`, `left` what was.
fn settled_as(hub: &crate::Hub, id: u64, resident: &ResidentId, fate: Fate, left: Option<String>) {
    let mut store = hub.edit_store();
    let Some(index) = store.book.purges.kept.iter().position(|purge| purge.id == id) else { return };
    let old = store.book.purges.kept[index].clone();
    let mut purge = Purge::clone(&old);
    if let Some(item) = purge.items.get_mut(resident) {
        item.fate = fate;
        if left.is_some() {
            item.left = left;
        }
    }
    if !purge.items.values().any(|item| matches!(item.fate, Fate::Going | Fate::GoneThere)) {
        purge.step = Step::Done;
        hub.log.line(format_args!("the purge {id} of {}'s assistance is done", purge.account));
    }
    store.book.purges.kept[index] = Arc::new(purge);
    store.book_edit = crate::store::BookEdit::Full;
    if let Err(error) = store.save_book() {
        // Not kept: settled again as purges are looked at again.
        store.book.purges.kept[index] = old;
        hub.log.line(format_args!("keeping that {resident} of the purge {id} went: {error}"));
    }
}

/// A purge whose machine left the hub asks it nothing more: what is there
/// cannot be reached, and is said so. Whether any changed.
pub(crate) fn prune(store: &mut crate::store::Store) -> bool {
    let mut changed = false;
    let directory = store.book.directory.clone();
    let present = |key: &PublicKey| directory.members.iter().any(|member| member.key == *key);
    for purge in &mut store.book.purges.kept {
        if !purge.open() {
            continue;
        }
        let gone = match purge.step {
            Step::Closing => !present(&purge.key),
            Step::Deleting => purge
                .items
                .values()
                .any(|item| item.fate == Fate::Going && item.host.as_ref().is_none_or(|host| !present(host))),
            Step::Done => false,
        };
        if !gone {
            continue;
        }
        let purge = Arc::make_mut(purge);
        match purge.step {
            Step::Closing => {
                purge.step = Step::Done;
                purge.waiting = Some("its machine left the hub before it closed the assistance".into());
            }
            _ => {
                for item in purge.items.values_mut() {
                    if item.fate == Fate::Going && item.host.as_ref().is_none_or(|host| !present(host)) {
                        item.fate = Fate::Unreachable;
                    }
                }
                if !purge.items.values().any(|item| matches!(item.fate, Fate::Going | Fate::GoneThere)) {
                    purge.step = Step::Done;
                }
            }
        }
        changed = true;
    }
    changed
}

/// How often what purges ask is asked again of a machine that has not
/// answered it: one that never received it, or whose answer was lost.
const AGAIN: std::time::Duration = std::time::Duration::from_secs(10 * 60);

/// Asks again, every `AGAIN`, what each purge asks; lets go of machines
/// that left; runs as long as the hub.
pub(crate) async fn again(hub: std::sync::Weak<crate::Hub>) {
    loop {
        tokio::time::sleep(AGAIN).await;
        let Some(hub) = hub.upgrade() else { return };
        hub.blocking(|hub| {
            // Counted until its last write, so freezing waits, or nothing.
            let Some(_entered) = crate::moving::Moving::enter(&hub) else { return };
            if hub.store().book.purges.kept.iter().any(|purge| purge.open()) {
                let mut store = hub.edit_store();
                if prune(&mut store) {
                    store.book_edit = crate::store::BookEdit::Full;
                    if let Err(error) = store.save_book() {
                        hub.log.line(format_args!("keeping the purges whose machines left: {error}"));
                    }
                }
                drop(store);
                settle(&hub);
                tell(&hub, None);
            }
        })
        .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fastctx_net::wire::BornUnder;
    use fastctx_types::owned_records::Conversation;

    const OPENED: &str = "2026-11-01T00:00:00Z";
    const ENDED: &str = "g-00000000000000aa";

    fn directory() -> fastctx_net::directory::Directory {
        serde_json::from_value(serde_json::json!({
            "accounts": [{"name": "carol", "level": "member", "locked": false, "opened": OPENED}],
            "members": [],
            "projects": [],
        }))
        .expect("a directory")
    }

    fn grant(text: &str) -> GrantId {
        GrantId::try_from(text.to_string()).expect("a grant")
    }

    fn purge() -> Purge {
        Purge {
            id: 1,
            grants: vec![grant(ENDED)],
            account: "carol".parse().expect("an account"),
            generation: OPENED.parse().expect("a time"),
            machine: "host".parse().expect("a machine"),
            key: PublicKey::from_bytes([7; 32]),
            agent: "codex".parse().expect("an agent"),
            home: None,
            at: OPENED.parse().expect("a time"),
            by: None,
            round: 1,
            step: Step::Closing,
            items: BTreeMap::new(),
            waiting: None,
            audience: BTreeSet::new(),
            audience_named: BTreeSet::new(),
        }
    }

    fn resident(name: &str) -> ResidentId {
        name.parse().expect("a resident")
    }

    fn born(name: &str) -> BornUnder {
        BornUnder { resident: resident(name), colleague: false, others: false, born_at: None }
    }

    /// A computer named `name`, renamed from each of `was`, whose key is
    /// `byte` repeated.
    fn member(name: &str, was: &[&str], byte: u8) -> Arc<fastctx_net::directory::Member> {
        Arc::new(fastctx_net::directory::Member {
            name: name.parse().expect("a machine"),
            was: was.iter().map(|name| name.parse().expect("a machine")).collect(),
            key: PublicKey::from_bytes([byte; 32]),
            kind: fastctx_net::directory::Kind::Computer,
            logins: Default::default(),
            home: None,
            managed: false,
            code: None,
            group: None,
            direct: Vec::new(),
            relay: Vec::new(),
            added: OPENED.parse().expect("a time"),
            online: false,
            since: None,
            through: None,
            reach: None,
            paths: None,
            strain: None,
            workbench: None,
            numbers: None,
            settled: None,
        })
    }

    fn record(name: &str, machine: &str, under: &str) -> Conversation {
        let mut conversation = Conversation {
            account: "carol".parse().expect("an account"),
            address: fastctx_types::Address { machine: machine.parse().expect("a machine"), resident: resident(name) },
            shared_on: None,
            born_seal: None,
            previous: Vec::new(),
            identity: None,
            birth: Default::default(),
        };
        conversation.birth.generation = Some(OPENED.parse().expect("a time"));
        conversation.birth.assisted = true;
        conversation.birth.born_under = Some(grant(under));
        conversation
    }

    #[test]
    fn only_a_conversation_found_born_there_alone_never_moved_goes_whole() {
        let opened: Timestamp = OPENED.parse().expect("a time");
        let account: AccountName = "carol".parse().expect("an account");
        let mut owners = crate::owned_records::index::Index::default();
        let take = |owners: &mut crate::owned_records::index::Index, conversation: Conversation| {
            let key = conversation.address.resident.to_string();
            owners.take(&account, opened, &key, Some(conversation));
        };
        take(&mut owners, record("codex-0001", "host", ENDED));
        let mut moved = record("codex-0002", "laptop", ENDED);
        moved.previous.push("host".parse().expect("a machine"));
        take(&mut owners, moved);
        let mut colleague = record("codex-0003", "host", ENDED);
        colleague.birth.colleague = true;
        take(&mut owners, colleague);
        take(&mut owners, record("codex-0004", "host", "g-00000000000000bb"));
        // Registered as born under it, not found born there.
        take(&mut owners, record("codex-0005", "laptop", ENDED));
        let found = vec![
            born("codex-0001"),
            born("codex-0002"),
            born("codex-0003"),
            born("codex-0004"),
            BornUnder { colleague: true, ..born("codex-0006") },
            BornUnder { others: true, ..born("codex-0007") },
            // Not registered yet: its machine's word is enough.
            born("codex-0008"),
        ];
        // Its machine, and another, by their keys.
        let mut directory = directory();
        directory.members.push(member("host", &[], 7));
        directory.members.push(member("laptop", &[], 8));
        let items = decide(&fastctx_types::access::HubId::from_bytes([7; 8]), &directory, &owners, &purge(), found);
        let fate = |name: &str| items.get(&resident(name)).map(|item| item.fate.clone());
        assert_eq!(fate("codex-0001"), Some(Fate::Going));
        assert_eq!(fate("codex-0008"), Some(Fate::Going));
        for kept in ["codex-0002", "codex-0003", "codex-0004", "codex-0005", "codex-0006", "codex-0007"] {
            assert!(matches!(fate(kept), Some(Fate::Kept { .. })), "{kept} is kept");
        }
        // Each goes where it was born, by that machine's key.
        assert_eq!(items[&resident("codex-0001")].host, Some(PublicKey::from_bytes([7; 32])));
    }

    #[test]
    fn a_conversation_that_ever_had_another_address_is_kept_whatever_its_names_stand_for_now() {
        let opened: Timestamp = OPENED.parse().expect("a time");
        let account: AccountName = "carol".parse().expect("an account");
        let mut owners = crate::owned_records::index::Index::default();
        let take = |owners: &mut crate::owned_records::index::Index, conversation: Conversation| {
            let key = conversation.address.resident.to_string();
            owners.take(&account, opened, &key, Some(conversation));
        };
        let machine = |name: &str| -> MachineName { name.parse().expect("a machine") };
        // Born on the host, moved to a laptop and back; the laptop then left
        // the network and the host took its name: both names are the host's.
        let mut there_and_back = record("codex-0001", "host", ENDED);
        there_and_back.previous = vec![machine("host"), machine("laptop")];
        take(&mut owners, there_and_back);
        // An earlier address a build before recorded as its machine was
        // renamed: not told apart from a move, and kept as one.
        let mut renamed_before = record("codex-0002", "laptop", ENDED);
        renamed_before.previous = vec![machine("host")];
        take(&mut owners, renamed_before);
        // Recorded under the name its machine had, no address before it.
        take(&mut owners, record("codex-0003", "host", ENDED));
        // Recorded under the name its machine has now.
        take(&mut owners, record("codex-0004", "laptop", ENDED));
        let mut directory = directory();
        directory.members.push(member("laptop", &["host"], 7));
        let found = vec![born("codex-0001"), born("codex-0002"), born("codex-0003"), born("codex-0004")];
        let items = decide(&fastctx_types::access::HubId::from_bytes([7; 8]), &directory, &owners, &purge(), found);
        let fate = |name: &str| items.get(&resident(name)).map(|item| item.fate.clone());
        for kept in ["codex-0001", "codex-0002"] {
            assert!(matches!(fate(kept), Some(Fate::Kept { .. })), "{kept} is kept");
        }
        for going in ["codex-0003", "codex-0004"] {
            assert_eq!(fate(going), Some(Fate::Going), "{going} goes whole");
        }
    }

    #[test]
    fn a_purge_is_asked_of_the_machine_that_gave_it_then_of_each_conversation_going() {
        let mut purge = purge();
        let told = asked(&purge);
        assert_eq!(told.len(), 1);
        assert!(matches!(told[0].1.step, fastctx_net::wire::PurgeStep::Close));
        purge.step = Step::Deleting;
        purge.round = 2;
        let host = PublicKey::from_bytes([7; 32]);
        purge.items.insert(
            resident("codex-0001"),
            Item { host: Some(host), previous: Vec::new(), fate: Fate::Going, identity: None, left: None },
        );
        purge.items.insert(
            resident("codex-0002"),
            Item {
                host: Some(host),
                previous: Vec::new(),
                fate: Fate::Kept { why: "moved".into() },
                identity: None,
                left: None,
            },
        );
        let told = asked(&purge);
        assert_eq!(told.len(), 1);
        assert_eq!(told[0].1.round, 2);
        match &told[0].1.step {
            fastctx_net::wire::PurgeStep::Delete { residents } => assert_eq!(*residents, vec![resident("codex-0001")]),
            other => panic!("asked {other:?}"),
        }
        purge.step = Step::Done;
        assert!(asked(&purge).is_empty());
    }
}
