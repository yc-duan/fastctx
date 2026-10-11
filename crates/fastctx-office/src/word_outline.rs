//! Locate native headings in the existing Word Markdown representation.

use crate::explore::Catalogue;
use anyhow::{Result, bail};
use office_oxide::docx::{
    DocxDocument,
    document::BlockElement,
    paragraph::{ParagraphContent, Run, RunContent},
};
use std::collections::{HashSet, VecDeque};

pub(crate) fn collect(document: &office_oxide::Document, text: &str) -> Result<Catalogue> {
    // A marker changes no line breaks and cannot occur in the source.
    // Render one block at a time, including table cells and text boxes.
    let present: HashSet<usize> = text
        .split('\u{E000}')
        .skip(1)
        .filter_map(|part| {
            let (number, _) = part.split_once('\u{E001}')?;
            number.parse().ok().filter(|n| *n <= text.len())
        })
        .collect();
    let number = (0..=text.len()).find(|n| !present.contains(n)).expect("more candidates than occurrences");
    let marker = format!("\u{E000}{number}\u{E001}");
    let Some(docx) = document.as_docx() else {
        return legacy(document, text, &marker);
    };
    let mut probe = DocxDocument {
        body: Default::default(),
        styles: docx.styles.clone(),
        numbering: docx.numbering.clone(),
        theme: None,
        sections: Vec::new(),
        headers_footers: Vec::new(),
        unreadable_parts: Vec::new(),
        embedded_fonts: Vec::new(),
        images: Default::default(),
        core_properties: None,
        app_properties: None,
        package_properties: Default::default(),
        has_macros: false,
        footnotes: Vec::new(),
        endnotes: Vec::new(),
        comments: Vec::new(),
    };
    let mut lines = text.lines().enumerate();
    let mut result = Catalogue::new();
    let mut headers = HashSet::new();
    for header in docx.headers_footers.iter().filter(|h| h.active && h.is_header) {
        probe.body.elements = header.content.clone();
        let original = probe.to_markdown();
        if !original.trim().is_empty() && headers.insert(original.trim().to_owned()) {
            mark_docx(&mut probe.body.elements, docx, &marker);
            scan(&probe.to_markdown(), &mut lines, &mut result, &marker)?;
        }
    }
    for element in &docx.body.elements {
        probe.body.elements = vec![element.clone()];
        mark_docx(&mut probe.body.elements, docx, &marker);
        scan(&probe.to_markdown(), &mut lines, &mut result, &marker)?;
    }
    for note in docx.footnotes.iter().chain(&docx.endnotes) {
        probe.body.elements = note.content.clone();
        mark_docx(&mut probe.body.elements, docx, &marker);
        scan(&probe.to_markdown(), &mut lines, &mut result, &marker)?;
    }
    probe.body.elements.clear();
    for note in &docx.comments {
        probe.comments = vec![note.clone()];
        mark_docx(&mut probe.comments[0].content, docx, &marker);
        scan(&probe.to_markdown(), &mut lines, &mut result, &marker)?;
    }
    probe.comments.clear();
    let mut footers = HashSet::new();
    for footer in docx.headers_footers.iter().filter(|h| h.active && !h.is_header) {
        probe.body.elements = footer.content.clone();
        let original = probe.to_markdown();
        if !original.trim().is_empty() && footers.insert(original.trim().to_owned()) {
            mark_docx(&mut probe.body.elements, docx, &marker);
            scan(&probe.to_markdown(), &mut lines, &mut result, &marker)?;
        }
    }
    Ok(result)
}

fn mark_docx(elements: &mut [BlockElement], document: &DocxDocument, marker: &str) {
    for element in elements {
        match element {
            BlockElement::Table(table) => {
                for row in &mut table.rows {
                    for cell in &mut row.cells {
                        mark_docx(&mut cell.content, document, marker);
                    }
                }
            }
            BlockElement::Paragraph(paragraph) => {
                let heading = paragraph.properties.as_ref().is_some_and(|p| {
                    p.outline_level
                        .or_else(|| {
                            p.style_id.as_deref().and_then(|id| document.styles.as_ref()?.resolve_outline_level(id))
                        })
                        .is_some_and(|level| level < 9)
                });
                for content in &mut paragraph.content {
                    let runs = match content {
                        ParagraphContent::Run(run) => std::slice::from_mut(run),
                        ParagraphContent::Hyperlink(link) => &mut link.runs,
                    };
                    for run in runs {
                        for content in &mut run.content {
                            if let RunContent::TextBox(blocks) = content {
                                mark_docx(blocks, document, marker);
                            }
                        }
                    }
                }
                if heading {
                    paragraph.content.insert(
                        0,
                        ParagraphContent::Run(Run {
                            properties: Some(office_oxide::docx::formatting::RunProperties {
                                bold: Some(false),
                                italic: Some(false),
                                strike: Some(false),
                                dstrike: Some(false),
                                vertical_align: Some(office_oxide::docx::formatting::VerticalAlign::Baseline),
                                hidden: Some(false),
                                script: Some(Box::new(office_oxide::docx::formatting::ScriptRunProperties {
                                    bold_cs: Some(false),
                                    italic_cs: Some(false),
                                    ..Default::default()
                                })),
                                ..Default::default()
                            }),
                            content: vec![RunContent::Text(marker.into())],
                        }),
                    );
                }
            }
        }
    }
}

fn legacy(document: &office_oxide::Document, text: &str, marker: &str) -> Result<Catalogue> {
    let ir = document.to_ir();
    let mut surface = office_oxide::ir::DocumentIR::default();
    let mut lines = text.lines().enumerate();
    let mut result = Catalogue::new();
    for section in &ir.sections {
        let headers = [section.first_page_header.as_ref(), section.header.as_ref(), section.even_page_header.as_ref()];
        let footers = [section.first_page_footer.as_ref(), section.footer.as_ref(), section.even_page_footer.as_ref()];
        for elements in headers
            .into_iter()
            .flatten()
            .map(|h| &h.content)
            .chain(std::iter::once(&section.elements))
            .chain(footers.into_iter().flatten().map(|h| &h.content))
        {
            for element in elements {
                let mut element = element.clone();
                mark_ir(std::slice::from_mut(&mut element), marker);
                surface.sections = vec![office_oxide::ir::Section { elements: vec![element], ..Default::default() }];
                scan(&surface.to_markdown(), &mut lines, &mut result, marker)?;
            }
        }
    }
    Ok(result)
}

fn mark_ir(elements: &mut [office_oxide::ir::Element], marker: &str) {
    use office_oxide::ir::{Element, InlineContent, TextSpan};
    for element in elements {
        match element {
            Element::Heading(heading) => {
                heading.content.insert(0, InlineContent::Text(TextSpan { text: marker.into(), ..Default::default() }));
            }
            Element::Table(table) => {
                for row in &mut table.rows {
                    for cell in &mut row.cells {
                        mark_ir(&mut cell.content, marker);
                    }
                }
            }
            Element::TextBox(textbox) => mark_ir(&mut textbox.content, marker),
            Element::Footnote(note) | Element::Endnote(note) => mark_ir(&mut note.content, marker),
            Element::List(list) => {
                for item in &mut list.items {
                    mark_ir(&mut item.content, marker);
                }
            }
            _ => {}
        }
    }
}

fn scan(
    rendered: &str,
    lines: &mut std::iter::Enumerate<std::str::Lines<'_>>,
    result: &mut Catalogue,
    marker: &str,
) -> Result<()> {
    let expected: Vec<_> = rendered.lines().collect();
    if expected.is_empty() {
        return Ok(());
    }
    let target: Vec<_> = expected.iter().map(|line| line.replace(marker, "")).collect();
    let mut prefixes = vec![0; target.len()];
    for i in 1..target.len() {
        let mut n = prefixes[i - 1];
        while n > 0 && !same_line(&target[i], &target[n]) {
            n = prefixes[n - 1];
        }
        if same_line(&target[i], &target[n]) {
            n += 1;
        }
        prefixes[i] = n;
    }
    // A whitespace-only first line can also match a block separator. Match the
    // complete block in one pass so that its headings keep their real positions.
    let mut recent = VecDeque::new();
    let mut matched = 0;
    for (index, line) in lines.by_ref() {
        recent.push_back((index, line));
        if recent.len() > target.len() {
            recent.pop_front();
        }
        while matched > 0 && !same_line(line, &target[matched]) {
            matched = prefixes[matched - 1];
        }
        if same_line(line, &target[matched]) {
            matched += 1;
        }
        if matched == target.len() {
            for (expected, (index, line)) in expected.iter().zip(recent) {
                record(expected, line, index, result, marker);
            }
            return Ok(());
        }
    }
    bail!("Word heading locations do not match the rendered content")
}

fn record(expected: &str, actual: &str, index: usize, result: &mut Catalogue, marker: &str) {
    for _ in expected.match_indices(marker) {
        result.push(format!("{}\t{}\n", index + 1, crate::output::visible(actual)));
    }
}

fn same_line(actual: &str, expected: &str) -> bool {
    fn ordinal(line: &str) -> &str {
        let text = line.trim_start();
        let n = text.bytes().take_while(u8::is_ascii_digit).count();
        if n > 0 && (text[n..].starts_with(". ") || text[n..].starts_with(") ")) { &text[n + 2..] } else { line }
    }
    // Containers trim block whitespace; isolated blocks restart list counters.
    let (actual, expected) = (ordinal(actual).trim_end(), ordinal(expected).trim_end());
    actual == expected || actual.split_whitespace().eq(expected.split_whitespace())
}
