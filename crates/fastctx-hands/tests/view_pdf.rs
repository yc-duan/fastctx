//! Smoke contract for the embedded PDF engine and the terminal image path.

use std::io::Write as _;

use fastctx_hands::run::window::{BUDGET, units, window};
use fastctx_hands::view::{Options, view};
use fastctx_types::run::OutputPiece;

fn pdf(pages: usize, content: &[u8]) -> Vec<u8> {
    let mut objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        format!(
            "<< /Type /Pages /Count {pages} /Kids [{}] >>",
            (0..pages).map(|i| format!("{} 0 R", 4 + i * 2)).collect::<Vec<_>>().join(" ")
        )
        .into_bytes(),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
    ];
    for i in 0..pages {
        objects.push(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 3 0 R >> >> /Contents {} 0 R >>", 5 + i * 2).into_bytes());
        let mut stream = format!("<< /Length {} >>\nstream\n", content.len()).into_bytes();
        stream.extend_from_slice(content);
        stream.extend_from_slice(b"\nendstream");
        objects.push(stream);
    }
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = vec![0];
    for (i, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        writeln!(bytes, "{} 0 obj", i + 1).unwrap();
        bytes.extend_from_slice(object);
        bytes.extend_from_slice(b"\nendobj\n");
    }
    let start = bytes.len();
    writeln!(bytes, "xref\n0 {}\n0000000000 65535 f ", offsets.len()).unwrap();
    for offset in &offsets[1..] {
        writeln!(bytes, "{offset:010} 00000 n ").unwrap();
    }
    writeln!(bytes, "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{start}\n%%EOF", offsets.len()).unwrap();
    bytes
}

#[test]
fn pdf_view_contract() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("pages.pdf");
    // Deliberately paint the lower line first; extraction must use position.
    std::fs::write(&path, pdf(3, b"BT /F1 12 Tf 72 680 Td (Second line) Tj 0 20 Td (First line) Tj ET")).unwrap();
    let options = Options { pages: Some((2, 2)), in_run: true, ..Default::default() };
    let mut output = Vec::new();
    let seen = view(std::slice::from_ref(&path), &options, &mut output).unwrap().seen;
    let text = String::from_utf8(output).unwrap();
    assert!(text.contains("PDF, 3 pages"), "{text}");
    assert!(text.contains("=== page 2 ("), "{text}");
    assert!(!text.contains("=== page 1 ("), "{text}");
    assert!(text.contains("1\tFirst line\n2\tSecond line\n"), "{text}");
    assert_eq!(seen.len(), 1);
    assert!(seen[0].sha256.is_none());
    assert!(seen[0].modified.is_some());

    let mut output = Vec::new();
    view(std::slice::from_ref(&path), &Options { image: true, ..options }, &mut output).unwrap();
    let log = temp.path().join("render.log");
    std::fs::write(&log, output).unwrap();
    let result = window(&log).unwrap();
    let images: Vec<_> = result.pieces.iter().filter(|piece| matches!(piece, OutputPiece::Image { .. })).collect();
    assert_eq!(images.len(), 1);
    assert!(matches!(images[0], OutputPiece::Image { mime, width: 1545, height: 2000, .. } if mime == "image/png"));

    let mut content = b"BT /F1 10 Tf 30 760 Td ".to_vec();
    for _ in 0..55 {
        content.extend_from_slice(b"(A page of text that must stop at the shared answer budget.) Tj 0 -13 Td ");
    }
    content.extend_from_slice(b"ET");
    std::fs::write(&path, pdf(200, &content)).unwrap();
    let mut output = Vec::new();
    view(&[path], &Options { pages: Some((1, 200)), ..Options::default() }, &mut output).unwrap();
    let text = String::from_utf8(output).unwrap();
    assert!(units(&text) <= BUDGET);
    // The budget ends inside a page or between pages; either way the rest
    // is named.
    assert!(
        text.contains("remaining text not shown") || text.contains("-200 (not shown)"),
        "{}",
        &text[text.len() - 600..]
    );
    assert!(text.trim_end().ends_with("-200 (not shown) ==="));
}

#[test]
fn mixed_pdf_baselines_preserve_body_order() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("directions.pdf");
    let content = b"BT /F1 12 Tf 0 1 -1 0 25 150 Tm (Left margin reads upward) Tj ET
BT /F1 12 Tf 0 -1 1 0 580 650 Tm (Right margin reads downward) Tj ET
BT /F1 12 Tf 72 650 Td (Second paragraph remains after the first paragraph in the body.) Tj ET
BT /F1 12 Tf 72 700 Td (First paragraph remains before the second paragraph in the body.) Tj ET
BT /F1 12 Tf -1 0 0 -1 400 50 Tm (Upside down footer) Tj ET";
    for quarter_turns in 0..4 {
        let angle = f64::from(quarter_turns) * std::f64::consts::FRAC_PI_2;
        let (sin, cos) = angle.sin_cos();
        let a = cos * 0.6;
        let b = sin * 0.6;
        let c = -b;
        let d = a;
        let mut rotated =
            format!("q {a} {b} {c} {d} {} {} cm\n", 306.0 - a * 306.0 - c * 396.0, 396.0 - b * 306.0 - d * 396.0)
                .into_bytes();
        rotated.extend_from_slice(content);
        rotated.extend_from_slice(b"\nQ");
        std::fs::write(&path, pdf(1, &rotated)).unwrap();
        let mut output = Vec::new();
        view(std::slice::from_ref(&path), &Options::default(), &mut output).unwrap();
        let output = String::from_utf8(output).unwrap();
        let lines: Vec<_> = output.lines().filter_map(|line| line.split_once('\t').map(|(_, text)| text)).collect();
        assert_eq!(
            &lines[..3],
            [
                "First paragraph remains before the second paragraph in the body.",
                "Second paragraph remains after the first paragraph in the body.",
                "Upside down footer",
            ],
            "rotation {quarter_turns}: {output}"
        );
        assert_eq!(lines.len(), 5, "{output}");
        assert!(lines[3..].contains(&"Left margin reads upward"), "{output}");
        assert!(lines[3..].contains(&"Right margin reads downward"), "{output}");
    }
}
