//! A document's font bindings and failures must not leak between page reads.

use fastctx_pdf::Document;
use std::io::Write;

fn stream(data: &[u8], dictionary: &str) -> Vec<u8> {
    let mut bytes = format!("<< /Length {} {dictionary} >>\nstream\n", data.len()).into_bytes();
    bytes.extend_from_slice(data);
    bytes.extend_from_slice(b"\nendstream");
    bytes
}

fn fixture() -> Vec<u8> {
    let content = b"BT /F1 12 Tf 72 720 Td (AAA) Tj ET /Fm Do BT /F1 12 Tf 72 640 Td (A) Tj ET";
    let objects = [
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 6 0 R >> /XObject << /Fm 10 0 R >> >> /Contents 9 0 R >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 7 0 R >> /XObject << /Fm 10 0 R >> >> /Contents 9 0 R >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 6 0 R >> /XObject << /Fm 10 0 R >> >> /Contents 9 0 R /Annots [<< /Type /Annot /Subtype /Widget /Rect [0 0 10 10] /AP << /N (invalid) >> >>] >>".to_vec(),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding << /Type /Encoding /BaseEncoding /WinAnsiEncoding /Differences [65 /A] >> >>".to_vec(),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding << /Type /Encoding /BaseEncoding /WinAnsiEncoding /Differences [65 /B] >> >>".to_vec(),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding << /Type /Encoding /BaseEncoding /WinAnsiEncoding /Differences [65 /C] >> >>".to_vec(),
        stream(content, ""),
        stream(b"BT /F2 12 Tf 72 680 Td (AAA) Tj ET", "/Type /XObject /Subtype /Form /BBox [0 0 612 792] /Resources << /Font << /F2 8 0 R >> >>"),
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

fn check(document: &Document, page: usize) {
    let text = document.text(page).unwrap();
    let expected = if page == 0 { ["AAA", "CCC", "A"] } else { ["BBB", "CCC", "B"] };
    assert_eq!(text.lines, expected);
    assert!(text.notes.is_empty());
}

#[test]
fn page_reads_keep_local_fonts_and_recover_after_an_error() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("session.pdf");
    std::fs::write(&path, fixture()).unwrap();
    let document = Document::open(path).unwrap();
    for page in [0, 1, 0, 1] {
        check(&document, page);
    }
    assert!(document.text(2).is_err());
    check(&document, 1);
    check(&document, 0);
    std::thread::scope(|threads| {
        for page in 0..2 {
            let document = &document;
            threads.spawn(move || check(document, page));
        }
    });
}
