//! Adopt only the current Codex guidance block from a portable installation.

use super::*;

pub(super) fn apply(file: File<'_>, body: &str, previous: Option<&Receipt>) -> Result<(Applied, bool), Error> {
    let original = file.bytes.unwrap_or_default();
    if previous.is_none()
        && let Some(range) = marker_range(original, BEGIN, END, file.path)?
    {
        let block = block(body, file.path)?;
        if original[range] != block {
            return Err(Error::at(
                file.path,
                "the shared fastctx block differs from this version's guidance; left it in place",
            ));
        }
        return Ok((
            Applied {
                bytes: original.to_vec(),
                receipt: Receipt {
                    path: file.path.into(),
                    original_existed: true,
                    block,
                    prefix: vec![],
                    suffix: vec![],
                },
            },
            true,
        ));
    }
    super::apply(file, body, previous).map(|applied| (applied, false))
}
