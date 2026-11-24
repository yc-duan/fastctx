//! A project's description: the agents working in it keep one coherent
//! account of the project as it stands, as part of its record. Each computer
//! keeping the project has it in `description.md` beside the ledger; `edit`
//! and `write` on that file write the record's next version, merged with one
//! written meanwhile, and the file is written again from the record whenever
//! it is read for a session.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use fastctx_types::project::{DESCRIPTION_FILE, MAX_DESCRIPTION_BYTES, Record};
use fastctx_types::write::{ChangeKind, FileChange, WriteOp, WriteOutcome, WriteResult};
use fastctx_types::{JobId, TownName, ipc::Failure};

use crate::Node;
use crate::town::caller::Caller;

/// How long a write keeps merging with versions written meanwhile.
const MERGING: std::time::Duration = std::time::Duration::from_secs(55);

fn failure(error: impl std::fmt::Display) -> Failure {
    Failure::new(error.to_string())
}

/// Where this computer keeps the description of `town`.
pub(crate) fn path(world: &fastctx_types::layout::WorldDir, town: &TownName) -> PathBuf {
    world.town(town).path().join(DESCRIPTION_FILE)
}

/// Writes the description file of `town` from `record` when it says
/// otherwise; written empty for a record without one, so that it can be
/// written.
pub(crate) fn keep_copy(
    world: &fastctx_types::layout::WorldDir,
    town: &TownName,
    record: &Record,
) -> std::io::Result<()> {
    let file = path(world, town);
    if !fastctx_platform::disk::is_dir(world.town(town).path()) {
        return Ok(());
    }
    fastctx_platform::publish_changed(&file, record.description.as_bytes(), fastctx_platform::Permissions::Private)
        .map(|_| ())
}

/// The town whose description file `path` is, on this machine.
fn locate(node: &Node, path: &Path) -> Result<Option<TownName>, Failure> {
    let path = fastctx_platform::local_path(path).map_err(failure)?;
    if path.file_name().is_none_or(|name| name != DESCRIPTION_FILE) {
        return Ok(None);
    }
    let Some(parent) = path.parent() else { return Ok(None) };
    for town in node.world.towns().map_err(failure)? {
        if crate::replicas::state::path_key(node.world.town(&town).path()) == crate::replicas::state::path_key(parent) {
            return Ok(Some(town));
        }
    }
    Ok(None)
}

/// An `edit` or `write` of a project's description file, as the next version
/// of its record; none when the write is of another file.
pub(crate) async fn intercept(
    node: &Arc<Node>,
    caller: &Caller,
    operation: &WriteOp,
    job: Option<&JobId>,
    town: Option<&TownName>,
) -> Result<Option<WriteOutcome>, Failure> {
    let paths = fastctx_hands::write::touched_paths(operation).map_err(failure)?;
    if matches!(operation, WriteOp::ApplyPatch(_)) {
        for path in &paths {
            if locate(node, path)?.is_some() {
                return Ok(Some(Err("a project's description is changed with write or edit; no part of this \
                     patch was applied"
                    .to_string()
                    .into())));
            }
        }
        return Ok(None);
    }
    let Some(path) = paths.first() else { return Ok(None) };
    let Some(project) = locate(node, path)? else { return Ok(None) };
    // Written as the caller, whose accounts must be of the project: this
    // machine's own standing in it is not theirs.
    super::access::check(node, &crate::around::acting_of(node, caller), &project)?;
    let Some((latest, _)) = read(node, &project).await? else {
        return Err(Failure::new(format!("no record of the project {project} is on this machine")));
    };
    let previous = latest.description.clone();
    if matches!(operation, WriteOp::Write(_))
        && !previous.is_empty()
        && let Caller::Resident(owner) = caller
    {
        let seen = crate::write::load_seen(&node.world.owner(&node.names(), owner).seen()).map_err(failure)?;
        let canonical = fastctx_hands::files::canonical_path(path).map_err(failure)?;
        match seen.get(&canonical).map(|seen| &seen.version) {
            Some(fastctx_types::files::SeenVersion::Full { sha256 })
                if fastctx_hands::files::content_hash(previous.as_bytes()) == *sha256 => {}
            Some(_) => {
                return Ok(Some(Err("the description changed since it was viewed; view it again before \
                     replacing it, or change part of it with edit"
                    .to_string()
                    .into())));
            }
            None => {
                return Ok(Some(Err("write replaces the whole description; view it with fastctx view before \
                     replacing it, or change part of it with edit"
                    .to_string()
                    .into())));
            }
        }
    }
    let proposed = String::from_utf8(
        fastctx_hands::write::candidate::prepare(operation, Some(previous.as_bytes())).map_err(failure)?,
    )
    .map_err(failure)?;
    let mut current = latest;
    let deadline = tokio::time::Instant::now() + MERGING;
    let after = loop {
        let text = if matches!(operation, WriteOp::Write(_)) {
            fastctx_net::plan_merge::try_merge(&previous, &current.description, &proposed).ok_or_else(|| {
                failure("the description changed elsewhere and this write conflicts with it; view it again")
            })?
        } else {
            String::from_utf8(
                fastctx_hands::write::candidate::prepare(operation, Some(current.description.as_bytes()))
                    .map_err(failure)?,
            )
            .map_err(failure)?
        };
        if text.len() > MAX_DESCRIPTION_BYTES {
            return Ok(Some(Err(format!(
                "the description would be {} bytes; it holds at most {MAX_DESCRIPTION_BYTES}, being given whole to \
                 every agent in the project: keep it to what the next one needs",
                text.len()
            )
            .into())));
        }
        let next = Record {
            version: current.version + 1,
            description: text.clone(),
            by: caller.writer(),
            on: Some(node.machine()),
            at: jiff::Timestamp::now(),
            ..current.clone()
        };
        match super::project::write(node, &project, &next).await {
            Ok(()) => break text,
            Err(error) if error.message.contains("already") && tokio::time::Instant::now() < deadline => {
                current = read(node, &project)
                    .await?
                    .map(|(record, _)| record)
                    .ok_or_else(|| Failure::new(format!("the record of the project {project} is gone")))?;
            }
            Err(error) => return Err(error),
        }
    };
    let file = self::path(&node.world, &project);
    if let Err(error) =
        fastctx_platform::publish_changed(&file, after.as_bytes(), fastctx_platform::Permissions::Private)
    {
        node.log.line(format_args!("keeping {}: {error}", file.display()));
    }
    let note = format!("{} {}: saved", operation.tool(), path.display());
    let mut result = WriteResult {
        note_prefix: note.len(),
        text: note,
        changes: vec![FileChange {
            fact: None,
            path: path.clone(),
            kind: if previous.is_empty() { ChangeKind::Added } else { ChangeKind::Modified },
            before_hash: Some(fastctx_hands::files::content_hash(previous.as_bytes())),
            after_hash: Some(fastctx_hands::files::content_hash(after.as_bytes())),
            encoding: Some("utf-8".into()),
            encoding_selection: None,
        }],
        changed_since_seen: Vec::new(),
    };
    if let Caller::Resident(owner) = caller {
        result = crate::write::record::record(
            &node.world,
            &node.names(),
            owner,
            path,
            Some(previous.as_bytes()),
            after.as_bytes(),
            job,
            town,
            result,
        )
        .map_err(failure)?;
    }
    Ok(Some(Ok(result)))
}

async fn read(node: &Arc<Node>, town: &TownName) -> Result<Option<(Record, fastctx_types::project::Stamp)>, Failure> {
    let (world, name) = (node.world.clone(), town.clone());
    fastctx_platform::task::spawn_blocking(move || super::read::current(&world, &name))
        .await
        .map_err(|error| Failure::new(format!("reading {town}'s ledger: {error}")))?
        .map_err(|error| Failure::new(format!("reading {town}'s ledger: {error}")))
}
