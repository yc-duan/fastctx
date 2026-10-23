//! Prepare bytes for a ledger-backed write without modifying its disk projection.

use super::blocks::{self, Block};
use crate::encoding::file::Source;
use fastctx_types::write::WriteOp;

pub fn prepare(op: &WriteOp, before: Option<&[u8]>) -> Result<Vec<u8>, String> {
    let (blocks, all, input, encoding) = match op {
        WriteOp::Write(request) => {
            return match before {
                Some(bytes) => Source::decode(bytes.to_vec(), request.encoding.as_deref())?.whole(&request.content),
                None => crate::encoding::explicit(request.encoding.as_deref().unwrap_or("utf-8"), &[])
                    .map_err(|error| error.to_string())?
                    .encode(&request.content)
                    .map_err(|error| error.to_string()),
            };
        }
        WriteOp::Edit(request) => (
            vec![Block { old: &request.old_string, new: &request.new_string, markers: None }],
            request.replace_all,
            None,
            request.encoding.as_deref(),
        ),
        WriteOp::EditBlocks(request) => (
            blocks::parse(&request.input)?,
            request.replace_all,
            Some(request.input.as_str()),
            request.encoding.as_deref(),
        ),
        WriteOp::ApplyPatch(_) => return Err("project memory is written with write and edit".into()),
    };
    let bytes = before.ok_or("the memory file does not exist")?;
    let mut document = Source::decode(bytes.to_vec(), encoding)?;
    if let Some(input) = input
        && let Some(reason) = blocks::ambiguity(&document.text, &blocks, input)
    {
        return Err(reason);
    }
    let mut ranges = Vec::new();
    for block in &blocks {
        super::edit::replace(&mut document, block, all, "--all", &mut ranges)?;
    }
    Ok(document.raw)
}
