//! Freeze the OOXML address/formula contract independently of a writer's
//! interpretation: ECMA-376 worksheet cells, shared formulas and ST_Xstring.

use std::io::Write;

use fastctx_office::{Format, Selection, read};
use zip::{ZipWriter, write::SimpleFileOptions};

#[test]
fn addresses_caches_and_continuations_survive_the_budget() {
    let file = tempfile::tempfile().unwrap();
    let mut zip = ZipWriter::new(file);
    let parts = [
        (
            "_rels/.rels",
            r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="book" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#,
        ),
        (
            "xl/workbook.xml",
            r#"<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="数据" sheetId="1" r:id="sheet" state="veryHidden"/></sheets></workbook>"#,
        ),
        (
            "xl/_rels/workbook.xml.rels",
            r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="sheet" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/></Relationships>"#,
        ),
    ];
    for (name, text) in parts {
        zip.start_file(name, SimpleFileOptions::default()).unwrap();
        zip.write_all(text.as_bytes()).unwrap();
    }
    zip.start_file("xl/worksheets/sheet1.xml", SimpleFileOptions::default()).unwrap();
    zip.write_all(br#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData>"#)
        .unwrap();
    zip.write_all("<row r=\"1\"><c r=\"A1\" t=\"inlineStr\"><is><t>中 &amp; 文_x000A__x005F_x0041__xD83D__xDE00_&#x85;&#x2028;</t></is></c><c r=\"B1\"><f t=\"shared\" si=\"0\"/><v>12</v></c><c r=\"D1\"><f t=\"array\" ref=\"D1:D4\">A1:A4*B1:B4</f><v>10</v></c><c r=\"E1\"><f t=\"shared\" si=\"1\" ref=\"E1:F2\">SUM(A:A)+SUM(1:1)</f><v>1</v></c></row><row r=\"2\"><c r=\"B2\"><f t=\"shared\" si=\"0\" ref=\"B1:B2\">IF(A2=&quot;中文&quot;,'数据'!$A2,0)</f><v>13</v></c><c r=\"F2\"><f t=\"shared\" si=\"1\"/><v>2</v></c></row>".as_bytes()).unwrap();
    for row in 3..=200 {
        write!(zip, "<row r=\"{row}\"><c r=\"C{row}\"><f>SUM(A1:B2)</f><v>25</v></c></row>").unwrap();
    }
    zip.write_all(b"</sheetData><mergeCells><mergeCell ref=\"A4:B6\"/></mergeCells></worksheet>").unwrap();
    let file = zip.finish().unwrap();
    let full = read(file.try_clone().unwrap(), Format::Xlsx, &Selection::default(), 150_000).unwrap();
    assert!(!full.stopped);
    assert!(full.text.contains("very hidden"));
    assert!(full.text.contains("merged A4:B6"));
    assert!(full.text.contains("array D1:D4"));
    assert!(
        full.text.contains("A1 \"中 & 文\\n_x0041_😀\\u0085\\u2028\"\tB1 =IF(A1=\"中文\",'数据'!$A1,0) → 12"),
        "{}",
        full.text
    );
    assert!(full.text.contains("C200 =SUM(A1:B2) → 25"));
    assert!(full.text.contains("D2 =A1:A4*B1:B4 → (no cached value)"));
    assert!(full.text.contains("F2 =SUM(B:B)+SUM(2:2) → 2"));

    let bounded = read(file.try_clone().unwrap(), Format::Xlsx, &Selection::default(), 1_000).unwrap();
    assert!(bounded.stopped);
    assert!(bounded.facts.len() + bounded.text.len() <= 1_000);
    assert!(bounded.text.contains("remaining selected content not shown"));
    assert!(bounded.text.contains("the rest starts at "), "{}", bounded.text);
    let selection =
        Selection { sheet: Some("数据".into()), range: Some("B1:C2".parse().unwrap()), ..Default::default() };
    let selected = read(file, Format::Xlsx, &selection, 1_000).unwrap();
    assert!(!selected.stopped);
    assert!(!selected.text.contains("A1 \""));
    assert!(selected.text.contains("B1 =IF(A1=\"中文\",'数据'!$A1,0) → 12"));
}
