use crate::{
    Selection,
    output::{Output, line_reserve},
};
use anyhow::{Result, bail};
use office_oxide::Document;
pub(crate) fn read(document: &Document, selection: &Selection, out: &mut Output) -> Result<()> {
    let text = content(document)?;
    out.reserve_facts(line_reserve().len());
    let lines = out.numbered(&text, selection.lines, |n, total| format!("the rest is lines {n}-{total}"));
    out.facts(lines.facts(selection.lines.is_some()));
    Ok(())
}

fn content(document: &Document) -> Result<String> {
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
    Ok(document.to_markdown())
}

pub(crate) fn explore(document: &Document, budget: usize, whole_budget: usize) -> Result<crate::Content> {
    let text = content(document)?;
    let whole = crate::explore::numbered(&text, budget.min(whole_budget), budget >= whole_budget);
    if !whole.stopped && whole.facts.len() + whole.text.len() <= whole_budget {
        return Ok(whole);
    }
    let head_budget = crate::explore::head_budget(budget, "outline", 0);
    let mut head = crate::explore::numbered(&text, head_budget, false);
    let headings = crate::word_outline::collect(document, &text)?;
    headings.append(&mut head, "outline", budget.min(whole_budget));
    head.stopped = (head_budget < crate::explore::HEAD && head.stopped) || head.text.is_empty();
    Ok(head)
}
