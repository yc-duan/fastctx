//! SQL is the context truth. The bounded files are a compatibility projection
//! for running Claude plugins, which continue to pair transient bodies by hash.

use super::storage::{Block, State};
use crate::records as families;
use fastctx_types::{
    ResidentId,
    layout::{ConversationDir, WorldDir},
};
use std::{collections::BTreeMap, io, path::Path, sync::LazyLock};

pub(super) const BLOCKS: &str = "context-blocks";
pub(super) const STATE: &str = "context-state";

fn prepare(world: &WorldDir, id: &ResidentId) -> io::Result<ConversationDir> {
    let directory = world.conversation(id);
    families::ensure_each(&directory, BLOCKS, |push| {
        for path in crate::retention::entries(&super::storage::directory(world, id))? {
            if path.extension().is_none_or(|extension| extension != "json") {
                continue;
            }
            let Some(name) = path.file_stem().and_then(|name| name.to_str()).filter(|name| super::storage::valid(name))
            else {
                continue;
            };
            let mut row: Block = serde_json::from_slice(&fastctx_platform::disk::read(&path)?)?;
            if row.transient {
                row.text.clear();
                row.fact.clear();
            }
            push(families::Source::new(name.into(), path.clone(), serde_json::to_vec(&row)?)?)?;
        }
        Ok(())
    })?;
    families::ensure_each(&directory, STATE, |push| {
        let path = super::storage::state_path(world, id);
        match fastctx_platform::disk::read(&path) {
            Ok(bytes) => {
                let state: State = serde_json::from_slice(&bytes)?;
                push(families::Source::new("current".into(), path, serde_json::to_vec(&state)?)?)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    })?;
    Ok(directory)
}

pub(super) fn blocks(rows: &families::Rows<'_>) -> fastctx_storage::Result<BTreeMap<String, Block>> {
    let mut query=rows.connection().prepare_cached("SELECT identity,body FROM conversation_records WHERE conversation=?1 AND family='context-blocks' ORDER BY identity LIMIT 13")?;
    let result = query
        .query_map([rows.conversation()], |row| Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?)))?
        .map(|row| {
            let (key, body) = row?;
            Ok((key, families::decode(&body)?))
        })
        .collect::<fastctx_storage::Result<BTreeMap<_, _>>>()?;
    if result.len() > 12 {
        return Err(fastctx_storage::Error::new(super::words::TOO_MANY));
    }
    Ok(result)
}
fn snapshot(directory: &ConversationDir) -> io::Result<(State, BTreeMap<String, Block>)> {
    families::read(directory, |rows| Ok((rows.get(STATE, "current")?.unwrap_or_default(), blocks(rows)?)))
}

pub(super) fn read<T>(
    world: &WorldDir,
    id: &ResidentId,
    action: impl FnOnce(State, BTreeMap<String, Block>) -> io::Result<T>,
) -> io::Result<T> {
    let directory = prepare(world, id)?;
    let (state, blocks) = snapshot(&directory)?;
    action(state, blocks)
}

pub(super) fn commit<T: Send + 'static>(
    world: &WorldDir,
    id: &ResidentId,
    bytes: usize,
    after: impl FnOnce(),
    action: impl FnOnce(&families::Rows<'_>) -> fastctx_storage::Result<T> + Send + 'static,
) -> io::Result<T> {
    let directory = prepare(world, id)?;
    crate::conversation_store::with_live_files(&directory, || {
        let result = families::write_at(&directory, fastctx_storage::Priority::Interactive, bytes, move |rows| {
            let result = action(rows)?;
            rows.connection().execute(
                "INSERT OR IGNORE INTO context_projection_dirty(conversation) VALUES(?1)",
                [rows.conversation()],
            )?;
            Ok(result)
        })?;
        after();
        if project_live(world, id, &directory).is_err() {
            notify(world);
        }
        Ok(result)
    })
}

pub(super) fn state<T: Send + 'static>(
    world: &WorldDir,
    id: &ResidentId,
    action: impl FnOnce(&families::Rows<'_>, &mut State) -> fastctx_storage::Result<T> + Send + 'static,
) -> io::Result<T> {
    commit(
        world,
        id,
        4096,
        || {},
        move |rows| {
            let mut state = rows.get(STATE, "current")?.unwrap_or_default();
            let result = action(rows, &mut state)?;
            rows.put(STATE, "current", &state)?;
            Ok(result)
        },
    )
}

fn project(world: &WorldDir, id: &ResidentId, directory: &ConversationDir) -> io::Result<()> {
    crate::conversation_store::with_live_files(directory, || project_live(world, id, directory))
}

fn project_live(world: &WorldDir, id: &ResidentId, directory: &ConversationDir) -> io::Result<()> {
    let (state, blocks) = snapshot(directory)?;
    fastctx_platform::ensure_private_dir(&super::storage::directory(world, id))?;
    for (name, block) in blocks {
        debug_assert!(!block.transient || (block.text.is_empty() && block.fact.is_empty()));
        publish_changed(
            &super::storage::directory(world, id).join(format!("{name}.json")),
            &serde_json::to_vec(&block)?,
        )?;
    }
    publish_changed(&super::storage::state_path(world, id), &serde_json::to_vec(&state)?)?;
    families::write_at(directory, fastctx_storage::Priority::Background, 64, |rows| {
        rows.connection()
            .execute("DELETE FROM context_projection_dirty WHERE conversation=?1", [rows.conversation()])?;
        Ok(())
    })
}
fn publish_changed(path: &Path, bytes: &[u8]) -> io::Result<()> {
    match fastctx_platform::disk::read(path) {
        Ok(previous) if previous == bytes => return Ok(()),
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    fastctx_platform::publish(path, bytes, fastctx_platform::Permissions::Private)
}

pub(super) fn initialized(world: &WorldDir, id: &ResidentId) -> io::Result<bool> {
    let directory = prepare(world, id)?;
    families::read(&directory, |rows| Ok(rows.get::<State>(STATE, "current")?.is_some()))
}

static SIGNALS: LazyLock<
    std::sync::Mutex<std::collections::HashMap<std::path::PathBuf, std::sync::Weak<tokio::sync::Notify>>>,
> = LazyLock::new(Default::default);
fn notify(world: &WorldDir) {
    if let Some(signal) =
        SIGNALS.lock().expect("context projection signals").get(world.path()).and_then(std::sync::Weak::upgrade)
    {
        signal.notify_one();
    }
}
/// Pairing needs a readable projection before handing a pointer to the plugin.
pub(super) fn repair(world: &WorldDir, id: &ResidentId) -> io::Result<()> {
    let directory = prepare(world, id)?;
    project(world, id, &directory)
}
pub(super) fn start(node: &std::sync::Arc<crate::Node>) {
    let signal = std::sync::Arc::new(tokio::sync::Notify::new());
    {
        let mut signals = SIGNALS.lock().expect("context projection signals");
        signals.retain(|_, signal| signal.upgrade().is_some());
        signals.insert(node.world.path().to_path_buf(), std::sync::Arc::downgrade(&signal));
    }
    let node = node.clone();
    fastctx_platform::task::spawn(async move {
        loop {
            let notified = signal.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let working = node.clone();
            let result=fastctx_platform::task::spawn_blocking(move ||->io::Result<bool> {
                let mut after=String::new();let mut failed=false;
                loop {
                    let keys=working.database.read_now(|db| {
                        let mut query=db.prepare_cached("SELECT conversation FROM context_projection_dirty WHERE conversation>?1 ORDER BY conversation LIMIT 16")?;
                        Ok(query.query_map([&after],|row|row.get::<_,String>(0))?.collect::<std::result::Result<Vec<_>,_>>()?)
                    }).map_err(io::Error::other)?;
                    let Some(last)=keys.last() else {break};after=last.clone();
                    for key in keys {
                        let result=key.parse::<ResidentId>().map_err(io::Error::other).and_then(|id|repair(&working.world,&id));
                        if let Err(error)=result {failed=true;working.log.line(format_args!("repairing context projection for {key}: {error}"));}
                    }
                }
                Ok(failed)
            }).await;
            let failed = match result {
                Ok(Ok(failed)) => failed,
                Ok(Err(error)) => {
                    node.log.line(format_args!("reading context projections to repair: {error}"));
                    true
                }
                Err(error) => {
                    node.log.line(format_args!("context projection repair worker failed: {error}"));
                    true
                }
            };
            if failed {
                tokio::select! {()=&mut notified=>{},()=tokio::time::sleep(std::time::Duration::from_secs(5))=>{}}
            } else {
                notified.await;
            }
        }
    });
}
