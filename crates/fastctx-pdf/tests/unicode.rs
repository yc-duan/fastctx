//! Frozen PDF Unicode contracts, independent of layout-engine output.

use fastctx_pdf::Document;
use std::io::Write;

fn stream(data: &[u8]) -> Vec<u8> {
    let mut object = format!("<< /Length {} >>\nstream\n", data.len()).into_bytes();
    object.extend_from_slice(data);
    object.extend_from_slice(b"\nendstream");
    object
}

fn fixture() -> Vec<u8> {
    let cmap = b"/CIDInit /ProcSet findresource begin 12 dict begin begincmap
/CMapType 2 def /CMapName /UnicodeContract def
1 begincodespacerange <0000> <FFFF> endcodespacerange
2 beginbfrange <0001> <0002> <00540068>
<2000> <FFFF> <0000> endbfrange
1 beginbfchar <0002> <0051> endbfchar
endcmap CMapName currentdict /CMap defineresource pop end end";
    let objects = [
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 4 0 R >> >> /Contents 7 0 R >>".to_vec(),
        b"<< /Type /Font /Subtype /Type0 /BaseFont /Contract /Encoding /Identity-H /DescendantFonts [5 0 R] /ToUnicode 6 0 R >>".to_vec(),
        b"<< /Type /Font /Subtype /CIDFontType2 /BaseFont /Contract /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> /DW 1000 >>".to_vec(),
        stream(cmap),
        stream(b"BT /F1 12 Tf 72 720 Td <000100028000> Tj ET"),
    ];
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = vec![0];
    for (index, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        writeln!(bytes, "{} 0 obj", index + 1).unwrap();
        bytes.extend_from_slice(object);
        bytes.extend_from_slice(b"\nendobj\n");
    }
    let xref = bytes.len();
    writeln!(bytes, "xref\n0 {}\n0000000000 65535 f ", offsets.len()).unwrap();
    for offset in &offsets[1..] {
        writeln!(bytes, "{offset:010} 00000 n ").unwrap();
    }
    writeln!(bytes, "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF", offsets.len()).unwrap();
    bytes
}

#[test]
fn unicode_ranges_keep_strings_overrides_and_distant_codes() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("unicode.pdf");
    std::fs::write(&path, fixture()).unwrap();
    let document = Document::open(path).unwrap();
    assert_eq!(document.page_count(), 1);
    let page = document.text(0).unwrap();
    // UTF-16BE string Th; later bfchar overrides Ti with Q; 0x8000 lies
    // 24576 codes into the declared range and maps to U+6000.
    assert_eq!(page.lines, ["ThQ\u{6000}"]);
    assert!(page.notes.is_empty(), "{:?}", page.notes);
}
