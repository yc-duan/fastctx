# fastctx-pdf

An independent Rust library for opening a local PDF, counting its pages,
extracting one page's reading-order text, and rendering one page as PNG.
It does not depend on other fastctx crates or require an external executable.

```rust,no_run
let document = fastctx_pdf::Document::open("paper.pdf")?;
let text = document.text(0)?; // Page indices are zero-based.
for line in text.lines {
    println!("{line}");
}
let image = document.render(0, 2000)?; // Long edge in pixels.
# Ok::<(), String>(())
```

Page results include notes about content that cannot be represented. Text
extraction retains invisible OCR text but does not perform OCR. Unmapped glyph
runs are shown as `[unmapped]`, with the original glyph count in the notes.
Encrypted files requiring a password return an error; owner-only encryption
that permits opening with an empty user password is supported.

The source is memory-mapped for Hayro; PDFOxide reads through a file-backed
reader. Text calls reuse the page index, parsed fonts and Unicode maps. Decoded
page and Form streams, encoded stream cache entries, and page-local font
bindings are released after each extraction. Rendering uses a fresh page
interpreter. Text extractions on one document are serialized; different
Documents can be read independently. One selected page is decoded as a unit.
The source must remain unchanged while
the document is open. Paths are ordinary native `Path` values; shell path
conventions and output budgets belong to the caller.

PDFOxide supplies original font-decoded glyph clusters through the local patch
documented in `vendor/pdf_oxide/FASTCTX-PATCH.md`. Content-stream adjacency and
exact advances form fragments; geometry joins fragments and orders complete
lines. Direction groups, bidirectional runs and combining marks are handled
before page reading order. Hayro renders with embedded fonts, or installed
substitutes selected by character coverage, declared CJK collection and name.
The library never installs fonts. Missing local glyph coverage is a page note.

The workspace uses Apache-2.0. Engine and asset notices are carried in
`src/NOTICE`, the vendored engine's `NOTICE`, and the generated release notices.
