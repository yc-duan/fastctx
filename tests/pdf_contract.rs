#![cfg(feature = "pdf")]

mod common;

use base64::Engine;
use common::{
    error_text, normalized, text, write, write_encrypted_pdf, write_pdf, write_pdf_with_media_box,
};
use fastctx::read_tool::{ReadRequest, read_file};
use fastctx::{ImageDetail, ToolContent};

fn request(path: &std::path::Path, pages: Option<&str>, mode: Option<&str>) -> ReadRequest {
    ReadRequest {
        file_path: normalized(path),
        offset: None,
        limit: None,
        pages: pages.map(str::to_string),
        pdf_mode: mode.map(str::to_string),
        encoding: None,
        view: None,
    }
}

#[test]
fn pdf_text_mode_returns_one_block_with_page_sections() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("two-pages.pdf");
    write_pdf(&path, &[Some("Page One"), Some("Page Two")]);

    assert_eq!(
        text(read_file(request(&path, None, None))),
        "=== Page 1 ===\nPage One\n\n=== Page 2 ===\nPage Two\n\n(Complete: pages 1-2 of 2 shown.)"
    );
}

#[test]
fn pdf_image_mode_returns_150_dpi_pngs_then_an_independent_terminal_block() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("two-pages.pdf");
    write_pdf(&path, &[Some("Page One"), Some("Page Two")]);

    let response = read_file(request(&path, None, Some("image")));
    assert!(!response.is_error, "{response:?}");
    assert_eq!(response.content.len(), 3);
    for content in &response.content[..2] {
        let ToolContent::Image {
            data,
            mime_type,
            detail,
        } = content
        else {
            panic!("expected an image before the terminal block");
        };
        assert_eq!(mime_type, "image/png");
        assert_eq!(*detail, Some(ImageDetail::High));
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(data)
            .unwrap();
        let image = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png).unwrap();
        assert_eq!((image.width(), image.height()), (1240, 1754));
    }
    assert_eq!(
        response.content[2],
        ToolContent::Text("(Complete: pages 1-2 of 2 rendered.)".to_string())
    );
}

#[test]
fn pdf_image_mode_defaults_to_four_pages_and_gives_a_self_sized_continuation() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("five-pages.pdf");
    write_pdf(&path, &[Some("page"); 5]);

    let response = read_file(request(&path, None, Some("image")));
    assert!(!response.is_error, "{response:?}");
    assert_eq!(response.content.len(), 5);
    assert!(
        response.content[..4]
            .iter()
            .all(|content| matches!(content, ToolContent::Image { .. }))
    );
    assert_eq!(
        response.content[4],
        ToolContent::Text(
            "(Partial: pages 1-4 of 5 rendered. Continue with pages=\"5\".)".to_string()
        )
    );
}

#[test]
fn pdf_page_limits_ranges_and_errors_use_exact_contract_text() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("eleven.pdf");
    write_pdf(&path, &[Some("page"); 11]);

    assert_eq!(
        error_text(read_file(request(&path, None, None))),
        "This PDF has 11 pages. Specify the pages parameter (e.g. \"1-10\"); max 20 pages per call."
    );
    assert_eq!(
        text(read_file(request(&path, Some("2-3"), None))),
        "=== Page 2 ===\npage\n\n=== Page 3 ===\npage\n\n(Partial: pages 2-3 of 11 shown. Continue with pages=\"4-5\".)"
    );
    assert_eq!(
        error_text(read_file(request(&path, Some("12"), None))),
        "Page range \"12\" is out of bounds: this PDF has 11 pages."
    );
    assert_eq!(
        error_text(read_file(request(&path, Some("3-1"), None))),
        "Invalid pages value \"3-1\". Use forms like \"3\", \"1-5\" (max 20 pages per call)."
    );
}

#[test]
fn malformed_pdf_page_ranges_all_use_the_same_actionable_shape() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("pages.pdf");
    write_pdf(&path, &[Some("one"), Some("two")]);
    for value in ["", "1-", "1-2-3", " 1", "2-1"] {
        assert_eq!(
            error_text(read_file(request(&path, Some(value), None))),
            format!(
                "Invalid pages value \"{value}\". Use forms like \"3\", \"1-5\" (max 20 pages per call)."
            )
        );
    }
    assert_eq!(
        error_text(read_file(request(&path, Some("0"), None))),
        "Page range \"0\" is out of bounds: this PDF has 2 pages."
    );
}

#[test]
fn pdf_mode_rejects_unknown_values_with_the_exact_guidance() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("page.pdf");
    write_pdf(&path, &[Some("page")]);
    assert_eq!(
        error_text(read_file(request(&path, None, Some("both")))),
        "Invalid pdf_mode value \"both\". Use \"text\" or \"image\"."
    );
}

#[test]
fn text_mode_marks_blank_pages_and_guides_an_all_blank_selection_to_image_mode() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("scanned.pdf");
    write_pdf(&path, &[None, None]);

    assert_eq!(
        text(read_file(request(&path, None, None))),
        "=== Page 1 === (no text layer)\n\n=== Page 2 === (no text layer)\n\n(Note: no text layer in the selected pages; use pdf_mode=\"image\" to view rendered pages.)\n(Complete: pages 1-2 of 2 shown.)"
    );
}

#[test]
fn blank_selection_is_labeled_from_selected_pages_not_unselected_pages() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("mixed.pdf");
    write_pdf(&path, &[None, Some("Text exists elsewhere")]);

    assert_eq!(
        text(read_file(request(&path, Some("1"), None))),
        "=== Page 1 === (no text layer)\n\n(Note: no text layer in the selected pages; use pdf_mode=\"image\" to view rendered pages.)\n(Partial: page 1 of 2 shown. Continue with pages=\"2\".)"
    );
}

#[test]
fn single_page_pdf_uses_mode_specific_singular_complete_notes() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("single.pdf");
    write_pdf(&path, &[Some("Only page")]);

    assert_eq!(
        text(read_file(request(&path, None, None))),
        "=== Page 1 ===\nOnly page\n\n(Complete: page 1 of 1 shown.)"
    );
    let image = read_file(request(&path, None, Some("image")));
    assert_eq!(
        image.content.last(),
        Some(&ToolContent::Text(
            "(Complete: page 1 of 1 rendered.)".to_string()
        ))
    );
}

#[test]
fn concurrent_pdf_reads_do_not_corrupt_each_others_documents() {
    use std::sync::{Arc, Barrier};

    let temp = tempfile::tempdir().unwrap();
    let paths = (0..4)
        .map(|index| {
            let path = temp.path().join(format!("concurrent-{index}.pdf"));
            write_pdf(&path, &[Some("Concurrent PDF")]);
            path
        })
        .collect::<Vec<_>>();
    let barrier = Arc::new(Barrier::new(paths.len()));
    std::thread::scope(|scope| {
        let mut handles = Vec::new();
        for path in &paths {
            let barrier = Arc::clone(&barrier);
            handles.push(scope.spawn(move || {
                barrier.wait();
                read_file(request(path, None, None))
            }));
        }
        for handle in handles {
            assert_eq!(
                text(handle.join().unwrap()),
                "=== Page 1 ===\nConcurrent PDF\n\n(Complete: page 1 of 1 shown.)"
            );
        }
    });
}

#[test]
fn pdf_rejects_more_than_twenty_selected_pages() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("twenty-one.pdf");
    write_pdf(&path, &[None; 21]);
    assert_eq!(
        error_text(read_file(request(&path, Some("1-21"), None))),
        "Invalid pages value \"1-21\". Use forms like \"3\", \"1-5\" (max 20 pages per call)."
    );
}

#[test]
fn encrypted_and_corrupted_pdfs_are_distinguished() {
    let temp = tempfile::tempdir().unwrap();
    let encrypted = temp.path().join("encrypted.pdf");
    write_encrypted_pdf(&encrypted);
    assert_eq!(
        error_text(read_file(request(&encrypted, None, None))),
        "Cannot read PDF: the file is password-protected."
    );

    let corrupted = temp.path().join("corrupted.pdf");
    write(&corrupted, b"%PDF-1.7\nnot a valid document");
    assert_eq!(
        error_text(read_file(request(&corrupted, None, None))),
        "Cannot read PDF: the file is corrupted or not a valid PDF."
    );
}

#[test]
fn image_mode_rejects_oversized_pages_before_bitmap_allocation() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("oversized-page.pdf");
    write_pdf_with_media_box(&path, &[Some("huge")], 100_000, 100_000);
    assert_eq!(
        error_text(read_file(request(&path, None, Some("image")))),
        "Cannot render PDF page 1: dimensions 208333x208333 pixels at 150 DPI exceed the rendering safety limits (max 16384 pixels per side and 32000000 pixels per page). Reduce the page size externally."
    );
    assert_eq!(
        text(read_file(request(&path, None, None))),
        "=== Page 1 ===\nhuge\n\n(Complete: page 1 of 1 shown.)"
    );
}

#[test]
fn selected_image_pages_have_a_combined_pixel_safety_limit() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("many-large-pages.pdf");
    write_pdf_with_media_box(&path, &[Some("page"); 15], 1_000, 1_000);
    assert_eq!(
        error_text(read_file(request(&path, Some("1-15"), Some("image")))),
        "Cannot render selected PDF pages: combined 150 DPI size exceeds the 64000000-pixel safety limit. Select fewer pages."
    );
}

#[test]
fn continuation_ranges_never_expand_and_cover_the_document_without_gaps() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("twenty-five.pdf");
    write_pdf(&path, &[Some("page"); 25]);

    for requested_width in [1_usize, 2, 10, 20] {
        let mut next_start = 1_usize;
        let mut seen = Vec::new();
        while next_start <= 25 {
            let next_end = (next_start + requested_width - 1).min(25);
            let range = if next_start == next_end {
                next_start.to_string()
            } else {
                format!("{next_start}-{next_end}")
            };
            let output = text(read_file(request(&path, Some(&range), None)));
            for page in next_start..=next_end {
                assert!(output.contains(&format!("=== Page {page} ===")));
                seen.push(page);
            }
            if next_end == 25 {
                assert!(output.ends_with(&format!(
                    "(Complete: {} of 25 shown.)",
                    page_span(next_start, next_end)
                )));
            } else {
                let following_end = (next_end + requested_width).min(25);
                let following = if next_end + 1 == following_end {
                    (next_end + 1).to_string()
                } else {
                    format!("{}-{following_end}", next_end + 1)
                };
                assert!(output.ends_with(&format!(
                    "(Partial: {} of 25 shown. Continue with pages=\"{following}\".)",
                    page_span(next_start, next_end)
                )));
            }
            next_start = next_end + 1;
        }
        assert_eq!(seen, (1..=25).collect::<Vec<_>>());
    }
}

fn page_span(first: usize, last: usize) -> String {
    if first == last {
        format!("page {first}")
    } else {
        format!("pages {first}-{last}")
    }
}
