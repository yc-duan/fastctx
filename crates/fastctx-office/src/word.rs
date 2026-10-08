use anyhow::{Result, bail};
use office_oxide::Document;

use crate::{Selection, output::Output};

pub(crate) fn read(document: &Document, selection: &Selection, out: &mut Output) -> Result<()> {
    if let Some(docx) = document.as_docx()
        && !docx.unreadable_parts.is_empty()
    {
        bail!("Word parts could not be read: {}", docx.unreadable_parts.join("; "));
    }
    if let Some(doc) = document.as_doc()
        && !doc.text_complete()
    {
        bail!("Word text is incomplete: {}", doc.warnings().join("; "));
    }
    let text = document.to_markdown();
    if out.line("", &format!("=== Word ({} lines) ===", text.lines().count()), "use --lines 1") {
        out.numbered(&text, selection.lines, None);
    }
    Ok(())
}
