#![cfg(feature = "pdf")]

mod common;

use common::{error_text, normalized, text, write_pdf, write_pdf_with_media_box};
use fastctx::read_tool::{ReadRequest, read_file};

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
            let output = text(handle.join().unwrap());
            assert!(output.starts_with("=== "), "{output}");
            assert!(
                output.ends_with("=== Page 1 ===\nConcurrent PDF"),
                "{output}"
            );
        }
    });
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
        format!(
            "=== {} (pages 1 of 1) ===\n=== Page 1 ===\nhuge",
            normalized(&path)
        )
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
