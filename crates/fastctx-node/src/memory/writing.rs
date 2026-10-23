//! Commit the World's ordinary write/edit commands through project memory.

use std::path::Path;
use std::sync::Arc;

use crate::{Node, town::caller::Caller};
use fastctx_hands::{files::canonical_path, memory_path::Target};
use fastctx_types::{
    Address, JobId, TownName,
    files::SeenVersion,
    ipc::Failure,
    memory::{Author, Evidence, Kind},
    project::Location,
    write::{ChangeKind, FileChange, WriteOp, WriteOutcome, WriteResult},
};

/// None means an ordinary file; a memory result is committed before it is returned.
pub(crate) async fn intercept(
    node: &Arc<Node>,
    caller: &Caller,
    op: &WriteOp,
    job: Option<&JobId>,
    job_town: Option<&TownName>,
) -> Result<Option<WriteOutcome>, Failure> {
    let path = match op {
        WriteOp::Edit(request) => &request.file_path,
        WriteOp::EditBlocks(request) => &request.file_path,
        WriteOp::Write(request) => &request.file_path,
        WriteOp::ApplyPatch(_) => return Ok(None),
    };
    let target = fastctx_hands::memory_path::locate(node.world.path(), path).map_err(Failure::new)?;
    let Some(target) = target else { return Ok(None) };
    let committed = commit(node, caller, &target, path, op, job, job_town).await;
    Ok(Some(committed.map_err(|failure| failure.message.into())))
}

async fn commit(
    node: &Arc<Node>,
    caller: &Caller,
    target: &Target,
    path: &Path,
    op: &WriteOp,
    job: Option<&JobId>,
    job_town: Option<&TownName>,
) -> Result<WriteResult, Failure> {
    let mut acting = crate::around::acting_of(node, caller);
    if let (Caller::Resident(owner), Some(job)) = (caller, job) {
        let record = fastctx_hands::run::record::read(&node.world.job_record(
            &node.names(),
            job_town,
            &owner.clone().into(),
            job,
        ))
        .map_err(|error| Failure::new(error.to_string()))?;
        if record.owner.resident() != Some(owner) || record.end.is_some() {
            return Err(Failure::refused(fastctx_types::memory::words::INACTIVE_JOB));
        }
        if let Some(account) = record.account.or_else(|| record.on.and_then(|on| on.account)) {
            acting = fastctx_types::access::Acting::Accounts { accounts: vec![account] };
        }
    }
    node.authorize_here(&acting)?;
    crate::town::access::ready(node, &acting, &target.town).await?;
    let author = super::identity::scoped(node, caller, &acting).await?;
    let (world, town) = (node.world.clone(), target.town.clone());
    let store = {
        let _syncing = node.syncing.lock().await;
        tokio::task::spawn_blocking(move || {
            super::projection::restore(&world, &town)?;
            super::store::read(&world, &town)
        })
        .await
        .map_err(|error| Failure::new(error.to_string()))?
        .map_err(|error| Failure::new(error.to_string()))?
    };
    let old = store.index.latest(&target.name);
    let before = if let Some(evidence) = &target.evidence {
        if store.index.evidence(&target.name).iter().any(|item| fastctx_net::ledger::conflicts(&item.name, evidence)) {
            return Err(Failure::new(fastctx_types::memory::words::IMMUTABLE_EVIDENCE));
        }
        if !old.is_some_and(|old| old.kind == Kind::Journal) {
            return Err(Failure::new(fastctx_types::memory::words::MISSING_JOURNAL));
        }
        if !matches!(op, WriteOp::Write(_)) {
            return Err(Failure::new(fastctx_types::memory::words::EVIDENCE_WRITE));
        }
        None
    } else {
        if target.kind == Kind::Journal && old.is_some() {
            return Err(Failure::new(fastctx_types::memory::words::IMMUTABLE_JOURNAL));
        }
        if old.is_some_and(|old| old.kind != target.kind) {
            return Err(Failure::new(fastctx_types::memory::words::OTHER_DOCUMENT));
        }
        old.map(|revision| {
            store
                .files
                .iter()
                .find(|file| file.path == revision.path("md"))
                .map(|file| file.bytes.0.as_slice())
                .ok_or_else(|| Failure::new(fastctx_types::memory::words::MISSING_SIGNED_TEXT))
        })
        .transpose()?
    };
    if let (Caller::Resident(owner), Some(before)) = (caller, before) {
        check_seen(node, owner, path, before)?;
    }
    let after = fastctx_hands::write::candidate::prepare(op, before).map_err(Failure::new)?;
    let (files, after) = match &target.evidence {
        Some(name) => evidence(node, target, name, after, author).await?,
        None => {
            let text =
                std::str::from_utf8(&after).map_err(|_| Failure::new(fastctx_types::memory::words::UTF8_REQUIRED))?;
            let (revision, files) = fastctx_net::memory::revision_files(
                target.kind,
                &target.name,
                text,
                old.map(|old| old.version),
                author,
            )
            .map_err(Failure::new)?;
            let stamped = files
                .iter()
                .find(|file| file.path == revision.path("md"))
                .expect("revision includes Markdown")
                .bytes
                .0
                .clone();
            (files, stamped)
        }
    };
    store.index.check(&files).map_err(Failure::new)?;
    super::api::publish(node, &acting, &target.town, files).await?;
    let text = fastctx_types::memory::words::saved(op.tool(), path.display());
    let mut result = WriteResult {
        note_prefix: text.len(),
        text,
        changes: vec![FileChange {
            path: path.to_path_buf(),
            kind: if before.is_some() { ChangeKind::Modified } else { ChangeKind::Added },
            before_hash: before.map(fastctx_hands::files::content_hash),
            after_hash: Some(fastctx_hands::files::content_hash(&after)),
            encoding: Some("utf-8".into()),
            encoding_selection: None,
            fact: None,
        }],
        changed_since_seen: Vec::new(),
    };
    if let Caller::Resident(owner) = caller {
        let owner = owner.clone();
        let (world, here, path, after, before, job, town) = (
            node.world.clone(),
            node.names(),
            path.to_path_buf(),
            after,
            before.map(<[u8]>::to_vec),
            job.cloned(),
            job_town.cloned(),
        );
        result = tokio::task::spawn_blocking(move || {
            record(&world, &here, &owner, &path, before.as_deref(), &after, job.as_ref(), town.as_ref(), result)
        })
        .await
        .map_err(|error| Failure::new(fastctx_types::memory::words::saved_unrecorded(error)))?
        .map_err(|error| Failure::new(fastctx_types::memory::words::saved_unrecorded(error)))?;
    }
    Ok(result)
}

fn check_seen(node: &Node, owner: &Address, path: &Path, before: &[u8]) -> Result<(), Failure> {
    let seen = crate::write::load_seen(&node.world.owner(&node.names(), owner).seen())
        .map_err(|error| Failure::new(error.to_string()))?;
    let canonical = canonical_path(path).map_err(|error| Failure::new(error.to_string()))?;
    if let Some(seen) = seen.get(&canonical) {
        let same = match &seen.version {
            SeenVersion::Full { sha256 } => *sha256 == fastctx_hands::files::content_hash(before),
            SeenVersion::Partial { size, modified } => std::fs::metadata(&canonical)
                .and_then(|metadata| Ok(*size == metadata.len() && *modified == metadata.modified()?))
                .map_err(|error| Failure::new(error.to_string()))?,
        };
        if !same {
            return Err(Failure::new(fastctx_types::memory::words::STALE_SPEC));
        }
    }
    Ok(())
}

async fn evidence(
    node: &Arc<Node>,
    target: &Target,
    name: &str,
    bytes: Vec<u8>,
    author: Author,
) -> Result<(Vec<fastctx_net::ledger::File>, Vec<u8>), Failure> {
    fastctx_net::ledger::check_path(name).map_err(Failure::new)?;
    let digest = fastctx_net::memory::digest(&bytes);
    let large = bytes.len() > fastctx_net::memory::INLINE_EVIDENCE_LIMIT;
    let location = if large {
        let retained = node.world.town(&target.town).attachments().join("memory").join(&target.name).join(&digest);
        let kept = bytes.clone();
        tokio::task::spawn_blocking(move || {
            fastctx_platform::ensure_private_dir(retained.parent().expect("has a parent"))?;
            match fastctx_platform::publish_new(&retained, &kept, fastctx_platform::Permissions::Private) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    if super::copy::fingerprint(&retained)? != (kept.len() as u64, fastctx_net::memory::digest(&kept)) {
                        return Err(std::io::Error::other(fastctx_types::memory::words::RETAINED_BYTES));
                    }
                }
                Err(error) => return Err(error),
            }
            super::copy::readonly(&retained)
        })
        .await
        .map_err(|error| Failure::new(error.to_string()))?
        .map_err(|error| Failure::new(error.to_string()))?;
        Some(Location {
            machine: node.machine(),
            path: crate::world::native(&node.world.town(&target.town).journals().join(&target.name).join(name)),
        })
    } else {
        None
    };
    let item = Evidence {
        journal: target.name.clone(),
        name: name.into(),
        bytes: bytes.len() as u64,
        sha256: digest,
        location,
        author,
    };
    let files = fastctx_net::memory::evidence_files(item, (!large).then(|| bytes.clone())).map_err(Failure::new)?;
    Ok((files, bytes))
}

#[allow(clippy::too_many_arguments)]
fn record(
    world: &fastctx_types::layout::WorldDir,
    here: &fastctx_types::layout::Names,
    owner: &Address,
    path: &Path,
    before: Option<&[u8]>,
    after: &[u8],
    job: Option<&JobId>,
    town: Option<&TownName>,
    mut result: WriteResult,
) -> std::io::Result<WriteResult> {
    if let Some(job) = job {
        let record_path = world.job_record(here, town, &owner.clone().into(), job);
        let record: fastctx_types::run::JobRecord = serde_json::from_slice(&std::fs::read(&record_path)?)?;
        if record.call_id.is_some() {
            let dir = record_path.parent().and_then(Path::parent).expect("job has a conversation");
            let diffs = fastctx_types::layout::ConversationDir::at(dir).job_scratch(job).join("diffs");
            let mut fact = fastctx_hands::write::capture_change(diffs, here.me.clone(), path, before, after)
                .map_err(std::io::Error::other)?;
            fact.note = Some(fastctx_types::run_facts::WriteNote::Text {
                command: fastctx_types::run_facts::NoteCommand::Write,
                text: format!("{}\n", result.text),
            });
            crate::run_facts::write(&record_path.with_extension("writes"), &fact)?;
            result.changes[0].fact = Some(Box::new(fact));
        }
    }
    crate::residents::record_seen(
        world,
        here,
        owner,
        &[fastctx_types::residents::Seen {
            path: canonical_path(path)?.to_string_lossy().into_owned(),
            at: jiff::Timestamp::now(),
            sha256: Some(fastctx_hands::files::content_hash(after)),
            size: after.len() as u64,
            modified: None,
            encoding: Some("utf-8".into()),
            encoding_selection: None,
        }],
    )?;
    Ok(result)
}
