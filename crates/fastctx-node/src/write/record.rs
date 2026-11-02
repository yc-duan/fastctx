//! Record a committed document edit in the same facts and seen-file stream as ordinary writes.

use fastctx_hands::files::canonical_path;
use fastctx_types::{Address, JobId, TownName, write::WriteResult};
use std::path::Path;

#[allow(clippy::too_many_arguments)]
pub(crate) fn record(
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
        let record: fastctx_types::run::JobRecord =
            serde_json::from_slice(&fastctx_platform::disk::read(&record_path)?)?;
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
