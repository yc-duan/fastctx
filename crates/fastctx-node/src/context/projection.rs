//! Publish only the records touched by a successful context transaction.
//! A pre-existing dirty mark has no per-block detail in the on-disk format,
//! so recovery still rebuilds the complete projection under its file gate.

use super::{
    records,
    storage::{Block, State},
};
use crate::records as families;
use fastctx_types::{
    ResidentId,
    layout::{ConversationDir, WorldDir},
};
use std::{io, path::Path, sync::LazyLock};

pub(super) enum Selection {
    All,
    State,
    Block(String),
}

pub(super) fn project(
    world: &WorldDir,
    id: &ResidentId,
    directory: &ConversationDir,
    selection: Selection,
) -> io::Result<()> {
    let (state, blocks) = families::read(directory, |rows| {
        let state = match &selection {
            Selection::Block(_) => None,
            _ => Some(rows.get::<State>(records::STATE, "current")?.unwrap_or_default()),
        };
        let blocks = match &selection {
            Selection::All => records::blocks(rows)?,
            Selection::State => Default::default(),
            Selection::Block(name) => {
                rows.get::<Block>(records::BLOCKS, name)?.map(|block| (name.clone(), block)).into_iter().collect()
            }
        };
        Ok((state, blocks))
    })?;
    if !blocks.is_empty() || matches!(selection, Selection::All) {
        fastctx_platform::ensure_private_dir(&super::storage::directory(world, id))?;
    }
    for (name, block) in blocks {
        debug_assert!(!block.transient || (block.text.is_empty() && block.fact.is_empty()));
        publish_changed(
            &super::storage::directory(world, id).join(format!("{name}.json")),
            &serde_json::to_vec(&block)?,
        )?;
    }
    if let Some(state) = state {
        publish_changed(&super::storage::state_path(world, id), &serde_json::to_vec(&state)?)?;
    }
    super::archived::clear(world, id, directory)?;
    families::write_at(directory, fastctx_storage::Priority::Background, 64, |rows| {
        rows.connection()
            .execute("DELETE FROM context_projection_dirty WHERE conversation=?1", [rows.conversation()])?;
        Ok(())
    })
}

fn publish_changed(path: &Path, bytes: &[u8]) -> io::Result<()> {
    static READ: LazyLock<&'static fastctx_platform::readings::Counter> =
        LazyLock::new(|| fastctx_platform::readings::counter("context.projection.checked"));
    static WRITTEN: LazyLock<&'static fastctx_platform::readings::Counter> =
        LazyLock::new(|| fastctx_platform::readings::counter("context.projection.written"));
    READ.add(1);
    match fastctx_platform::disk::read(path) {
        Ok(previous) if previous == bytes => return Ok(()),
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    fastctx_platform::publish(path, bytes, fastctx_platform::Permissions::Private)?;
    WRITTEN.add(1);
    Ok(())
}
