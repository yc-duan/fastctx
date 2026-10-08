use crate::{
    Selection,
    output::{Output, line_reserve},
};
use anyhow::{Result, bail};
use office_oxide::Document;
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
    out.reserve_facts(line_reserve().len());
    let lines = out.numbered(&text, selection.lines, |n, total| format!("use --lines {n}-{total}"));
    out.facts(lines.facts(selection.lines.is_some()));
    Ok(())
}
