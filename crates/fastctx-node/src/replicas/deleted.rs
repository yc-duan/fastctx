//! Conversations deleted here stay deleted (GitHub issue #58). A hub may
//! still list one as owned: when its deletion did not reach the hub, or came
//! after the list was read. None of its copies is made again here; those made
//! since are withdrawn with the list, the directory they left behind goes, and
//! the hub is told again of a deletion that finished here.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::{Path, PathBuf},
    sync::{
        Arc, LazyLock, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use fastctx_net::link::Link;
use fastctx_storage::rusqlite::OptionalExtension;
use fastctx_types::{
    Address, ResidentId,
    access::{Acting, HubId},
    layout::{Kind, WorldDir},
    owned_records::{Conversation, Request},
    replicas::{Catalog, Document, Scope},
};

use crate::Node;

/// What a conversation's directory holds that a copy made: its owner's mark.
const OWNER_MARK: &str = ".working-copy-owner.json";

/// What became of a deletion told again to a hub while this node runs.
enum Told {
    Taken,
    Failed(Instant),
}

/// The deletions told again to a hub while this node runs.
static TOLD: LazyLock<Mutex<HashMap<(HubId, ResidentId), Told>>> = LazyLock::new(Default::default);

/// Whether this run looked over every directory left of a deletion.
static SWEPT: AtomicBool = AtomicBool::new(false);

/// The resident of this machine `scope` belongs to, if it is a conversation's.
fn local(node: &Node, scope: &Scope) -> Option<ResidentId> {
    match scope {
        Scope::Conversation { conversation } if node.is_me(&conversation.address.machine) => {
            Some(conversation.address.resident.clone())
        }
        _ => None,
    }
}

/// Whether the conversation at `address` lived here and its deletion is
/// known without reading the database: a cheap check for each document.
pub(crate) fn known(node: &Node, address: &Address) -> bool {
    node.is_me(&address.machine)
        && !node.residents.knows(&address.resident)
        && node.residents.ever_deleted(&address.resident)
}

/// Whether `document` belongs to a conversation `known` deleted here.
pub(crate) fn document(node: &Node, document: &Document) -> bool {
    match document {
        Document::Plan { owner } => known(node, owner),
        Document::History { scope: Scope::Conversation { conversation }, .. } => known(node, &conversation.address),
        Document::History { .. } => false,
    }
}

/// The residents of this machine among `ids`, none living here now, whose
/// conversations were deleted here: whether each deletion finished, with
/// nothing of it let go whole to another machine before.
async fn deletions(node: &Arc<Node>, ids: BTreeSet<ResidentId>) -> Result<BTreeMap<ResidentId, bool>, String> {
    let ids: BTreeSet<ResidentId> = ids.into_iter().filter(|id| !node.residents.knows(id)).collect();
    if ids.is_empty() {
        return Ok(BTreeMap::new());
    }
    let (reading, asked) = (node.clone(), ids.clone());
    let mut found = fastctx_platform::task::blocking(move || {
        reading.database.read_now(|db| {
            let mut finished = db.prepare(
                "SELECT finished AND NOT EXISTS(SELECT 1 FROM node_conversation_releases WHERE name=?1) \
                 FROM node_conversation_deletions WHERE name=?1",
            )?;
            let mut found = BTreeMap::new();
            for id in &asked {
                if let Some(done) = finished.query_row([id.as_str()], |row| row.get::<_, bool>(0)).optional()? {
                    found.insert(id.clone(), done);
                }
            }
            Ok(found)
        })
    })
    .await
    .map_err(|error| error.to_string())?;
    // One deleted before the database kept deletions is known by its
    // tombstone alone, which does not say whether it finished.
    for id in ids {
        if node.residents.ever_deleted(&id) {
            found.entry(id).or_insert(false);
        }
    }
    Ok(found)
}

/// Whether the conversation at `address` lived here and was deleted here.
pub(crate) async fn here(node: &Arc<Node>, address: &Address) -> Result<bool, String> {
    if !node.is_me(&address.machine) {
        return Ok(false);
    }
    if known(node, address) {
        return Ok(true);
    }
    Ok(deletions(node, BTreeSet::from([address.resident.clone()])).await?.contains_key(&address.resident))
}

/// The conversations `catalog` lists that were deleted here, taken out of it
/// with their documents; what the hub said of each, and whether its deletion
/// finished.
pub(crate) async fn leave_out(node: &Arc<Node>, catalog: &mut Catalog) -> Result<Vec<(Conversation, bool)>, String> {
    let ids = catalog.scopes.iter().filter_map(|scope| local(node, scope)).collect();
    let gone = deletions(node, ids).await?;
    if gone.is_empty() {
        return Ok(Vec::new());
    }
    let away = |address: &Address| node.is_me(&address.machine) && gone.contains_key(&address.resident);
    let mut left = Vec::new();
    catalog.scopes.retain(|scope| match scope {
        Scope::Conversation { conversation } if away(&conversation.address) => {
            left.push((conversation.clone(), gone[&conversation.address.resident]));
            false
        }
        _ => true,
    });
    catalog.documents.retain(|head| match &head.document {
        Document::Plan { owner } => !away(owner),
        Document::History { scope: Scope::Conversation { conversation }, .. } => !away(&conversation.address),
        Document::History { .. } => true,
    });
    Ok(left)
}

/// How long a deletion the hub did not take is left before it is told again.
const TELL_AGAIN: Duration = Duration::from_secs(3600);

/// After the copies of `left` were withdrawn: tells the hub again of each
/// deletion that finished here, as the account that owns it where this
/// machine acts for that account, and takes away a directory a copy left.
/// Once a run, every directory left of a conversation deleted here is looked
/// at too, listed by the hub or not.
pub(crate) async fn settle(node: &Arc<Node>, link: &Link, left: Vec<(Conversation, bool)>) {
    if left.is_empty() && SWEPT.load(Ordering::Acquire) {
        return;
    }
    let acting = node.accounts_on(link, &Acting::Here);
    let hub = link.membership().id.clone();
    for (conversation, finished) in &left {
        let id = &conversation.address.resident;
        if !finished || !acting.contains(&conversation.account) {
            continue;
        }
        let key = (hub.clone(), id.clone());
        let due = match TOLD.lock().expect("never poisoned").get(&key) {
            Some(Told::Taken) => false,
            Some(Told::Failed(at)) => at.elapsed() >= TELL_AGAIN,
            None => true,
        };
        if !due {
            continue;
        }
        let told = match link.owned_records(&conversation.account, Request::Delete { resident: id.clone() }).await {
            Ok(_) => {
                node.log.line(format_args!(
                    "conversation {id} was deleted here but still owned at the hub; its deletion was told again"
                ));
                Told::Taken
            }
            Err(error) => {
                node.log.line(format_args!("telling the hub again that conversation {id} was deleted: {error}"));
                Told::Failed(Instant::now())
            }
        };
        TOLD.lock().expect("never poisoned").insert(key, told);
    }
    let mut ids: BTreeSet<ResidentId> =
        left.into_iter().map(|(conversation, _)| conversation.address.resident).collect();
    let (world, reading) = (node.world.clone(), node.clone());
    let swept = fastctx_platform::task::blocking(move || {
        if !SWEPT.swap(true, Ordering::AcqRel) {
            ids.extend(deleted_homes(&reading)?);
        }
        for id in ids {
            sweep(&world, &id)?;
        }
        Ok::<_, std::io::Error>(())
    })
    .await;
    if let Err(error) = swept {
        node.log.line(format_args!("taking away the directories copies left of deleted conversations: {error}"));
    }
}

/// The residents of this machine whose conversations were deleted here and
/// whose directories are still here.
fn deleted_homes(node: &Node) -> std::io::Result<Vec<ResidentId>> {
    let names: Vec<String> = node
        .database
        .read_now(|db| {
            let mut statement = db.prepare("SELECT name FROM node_conversation_deletions")?;
            let names = statement.query_map([], |row| row.get::<_, String>(0))?.collect::<Result<_, _>>()?;
            Ok(names)
        })
        .map_err(std::io::Error::other)?;
    Ok(names
        .into_iter()
        .filter_map(|name| match Kind::of(&name) {
            Some(Kind::Home(id)) if !node.residents.knows(&id) => Some(id),
            _ => None,
        })
        .filter(|id| fastctx_platform::disk::is_dir(node.world.conversation(id).path()))
        .collect())
}

/// Whether the directory at `path` holds nothing but what a copy of the
/// conversation `id` makes: its owner's mark naming it, an empty history
/// folder, an empty plan. What it holds, to take away, where so.
fn copies_only(path: &Path, id: &ResidentId) -> std::io::Result<Option<(Vec<PathBuf>, Vec<PathBuf>)>> {
    let entries = match fastctx_platform::disk::read_dir(path) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let (mut files, mut folders) = (Vec::new(), Vec::new());
    for entry in entries {
        let entry = entry?;
        let kind = fastctx_platform::disk::symlink_metadata(entry.path())?;
        match entry.file_name().to_str() {
            Some(OWNER_MARK) if kind.is_file() && marks(&entry.path(), id) => files.push(entry.path()),
            Some("plan.md") if kind.is_file() && kind.len() == 0 => files.push(entry.path()),
            Some("history") if kind.is_dir() && fastctx_platform::disk::read_dir(entry.path())?.next().is_none() => {
                folders.push(entry.path())
            }
            _ => return Ok(None),
        }
    }
    Ok(Some((files, folders)))
}

/// Whether the owner's mark at `path` names the conversation `id`.
fn marks(path: &Path, id: &ResidentId) -> bool {
    fastctx_platform::disk::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .is_some_and(|mark| mark["owner"]["resident"].as_str() == Some(id.as_str()))
}

/// Whether the directory of a conversation deleted here holds nothing but
/// what copies made again of it: it is not one to show.
pub(crate) fn left_of_copies(world: &WorldDir, id: &ResidentId) -> std::io::Result<bool> {
    let path = world.conversation(id).path().to_path_buf();
    Ok(!through_link(world, &path)? && copies_only(&path, id)?.is_some())
}

/// Whether `path`, or a folder above it below the World, is a link: nothing
/// is taken away through one.
fn through_link(world: &WorldDir, path: &Path) -> std::io::Result<bool> {
    let Ok(below) = path.strip_prefix(world.path()) else { return Ok(true) };
    let mut at = world.path().to_path_buf();
    for part in below.components() {
        at.push(part);
        match fastctx_platform::disk::symlink_metadata(&at) {
            Ok(metadata) if fastctx_platform::owned_directory::is_link(&metadata) => return Ok(true),
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error),
        }
    }
    Ok(false)
}

/// Takes away the directory of the conversation `id`, deleted here, when
/// copies made all it holds: never through a link.
fn sweep(world: &WorldDir, id: &ResidentId) -> std::io::Result<()> {
    let directory = world.conversation(id);
    crate::conversation_store::with_file_gate(&directory, || {
        let path = directory.path();
        if through_link(world, path)? {
            return Ok(());
        }
        let Some((files, folders)) = copies_only(path, id)? else { return Ok(()) };
        for file in files {
            fastctx_platform::disk::remove_file(file)?;
        }
        for folder in folders {
            fastctx_platform::disk::remove_dir(folder)?;
        }
        fastctx_platform::disk::remove_dir(path)
    })
}
