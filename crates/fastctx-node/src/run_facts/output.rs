//! Immutable ended-output measurements are reused by legacy progress frames.

use super::*;
use fastctx_types::run::JobRecord;
use fastctx_types::run_facts::OutputNoteMatch;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub(crate) struct Measured {
    started: jiff::Timestamp,
    writes_bytes: u64,
    output: OutputRef,
    notes: Vec<OutputNoteMatch>,
}

pub(super) fn ended(
    node: &Node,
    record_path: &Path,
    record: &JobRecord,
) -> io::Result<(OutputRef, Vec<OutputNoteMatch>)> {
    let metadata = super::writing::prepare(record_path)?;
    let writes_bytes = metadata.version("writes")?;
    if let Some(measured) = metadata.get::<Measured>("facts-output")?
        && measured.started == record.started_at
        && measured.writes_bytes == writes_bytes
        && measured.output.path == record.output
    {
        return Ok((measured.output, measured.notes));
    }
    let content = fastctx_hands::files::content_fingerprint(Path::new(&record.output))?;
    let index = fastctx_hands::lines::Index::open(
        Path::new(&record.output),
        node.world.path(),
        true,
        None,
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    )
    .map_err(io::Error::other)?;
    let writes = super::writes(record_path)?;
    let mut notes =
        fastctx_hands::run::projection::CapturedNotes::read(Path::new(&record.output), content.size, &writes)?
            .matches();
    super::frames::notes(&mut notes)?;
    let output = OutputRef {
        policy: Some(fastctx_hands::lines::output_policy()),
        machine: record.machine.clone(),
        path: record.output.clone(),
        bytes: content.size,
        lines: index.length,
        cut: false,
        content: Some(content),
    };
    let measured = Measured { started: record.started_at, writes_bytes, output, notes };
    metadata.put("facts-output", &measured)?;
    Ok((measured.output, measured.notes))
}
