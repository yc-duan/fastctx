use crate::{
    Selection,
    output::{Output, count, line_reserve},
};
use anyhow::{Result, bail};
use office_oxide::Document;

pub(crate) fn read(
    document: &Document,
    source: Option<std::fs::File>,
    selection: &Selection,
    out: &mut Output,
) -> Result<()> {
    if let Some(ppt) = document.as_ppt()
        && !ppt.text_complete()
    {
        bail!("PowerPoint text is incomplete: {}", ppt.warnings().join("; "));
    }
    let mut ir = document.to_ir();
    if ir.metadata.text_truncated {
        bail!("PowerPoint text is incomplete: {}", ir.metadata.warnings.join("; "));
    }
    let mut notes = source.map(crate::notes::Notes::open).transpose()?;
    let total =
        document.as_pptx().map(|d| d.slides.len()).or_else(|| document.as_ppt().map(|d| d.slides.len())).unwrap();
    let mut sections = std::mem::take(&mut ir.sections);
    if sections.len() < total {
        bail!("PowerPoint parser returned fewer slides than declared");
    }
    ir.sections = sections.split_off(total);
    let master = ir.plain_text();
    out.reserve_facts(format!("slides {n}-{n} of {n}; master and layout text only", n = u64::MAX).len());
    let (first, last) = selection.pages.unwrap_or((1, total as u64));
    let (mut shown_first, mut shown_last) = (0, 0);
    if !selection.master {
        for (i, mut section) in sections.into_iter().enumerate() {
            let page = i as u64 + 1;
            if page < first || page > last {
                continue;
            }
            section.title = None;
            let extra_notes = if let (Some(notes), Some(pptx)) = (&mut notes, document.as_pptx()) {
                section.speaker_notes = None;
                notes.slide(&pptx.presentation.slides[i].rel_id)?
            } else {
                String::new()
            };
            let hidden = section.hidden;
            ir.sections = vec![section];
            let mut text = ir.plain_text();
            if !extra_notes.trim().is_empty() {
                text.push_str("\n\n[Notes]\n");
                text.push_str(extra_notes.trim_end_matches('\n'));
            }
            let Some(header) = out.header(
                &format!("slide {page}"),
                &format!("{}; hidden", line_reserve()),
                &format!("the rest starts at slide {page}"),
            ) else {
                break;
            };
            if shown_first == 0 {
                shown_first = page;
            }
            shown_last = page;
            let lines =
                out.numbered(&text, selection.lines, |n, _| format!("the rest starts at slide {page}, line {n}"));
            let mut facts = lines.facts(selection.lines.is_some());
            if hidden {
                facts.push_str("; hidden");
            }
            out.update_header(header, &facts);
            if out.stopped() {
                break;
            }
        }
    }
    if !out.stopped()
        && (selection.master || selection.pages.is_none() || (first <= total as u64 && last >= total as u64))
        && (!master.is_empty() || selection.master)
        && let Some(header) =
            out.header("master and layout text", &line_reserve(), "the rest is the master and layout text")
    {
        let lines = out.numbered(&master, selection.lines, |n, _| {
            format!("the rest starts at line {n} of the master and layout text")
        });
        out.update_header(header, &lines.facts(selection.lines.is_some()));
    }
    let facts = if selection.master {
        format!("{}; master and layout text only", count(total as u64, "slide"))
    } else if total == 0 {
        count(0, "slide")
    } else if shown_first == 0 {
        format!("no slides shown of {total}")
    } else if selection.pages.is_some() || shown_last != total as u64 {
        if shown_first == shown_last {
            format!("slide {shown_first} of {total}")
        } else {
            format!("slides {shown_first}-{shown_last} of {total}")
        }
    } else {
        count(total as u64, "slide")
    };
    out.facts(facts);
    Ok(())
}
