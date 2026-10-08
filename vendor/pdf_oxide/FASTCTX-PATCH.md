# Local PDFOxide patch

Base: crates.io `pdf_oxide` **0.3.78**, MIT OR Apache-2.0 (used under Apache-2.0).

Source archive: <https://static.crates.io/crates/pdf_oxide/pdf_oxide-0.3.78.crate>

Archive SHA-256: `95b1ecc1a60a314fbd1aea5fe8f700ec32af1efa70fef5c6879162444da3437d`.

The workspace selects this copy through `[patch.crates-io]`. Its source,
normalized Cargo manifest, licenses, NOTICE, README and trademark notice come
from that release. Upstream examples, benchmarks and tests are not included.
These changes are local to fastctx and have not been submitted upstream.
The original font assets are unchanged. A complete Noto Emoji OFL text is
included alongside the upstream font-license summary; complete DejaVu/Arev
attribution is also carried in fastctx's supplemental notices.

The published character API sorts and deduplicates glyphs, inserts inferred
spaces, limits decoded strings, and does not expose the writing mode. Its
document opener also buffers the entire source for editing. The new read-only
API preserves the ingredients required by a caller that owns text layout.

- `document.rs`, `document/raw.rs`: file-backed reader and
  `PdfDocument::open_readonly`; `extract_raw_glyphs` visits page content and
  normal annotation appearances, including selected appearance states and
  their placement matrices. Raw extraction borrows the shared content buffer.
- `document/raw.rs`, `document/raw_cache.rs`: `release_page_data` discards
  decoded page/Form streams, raw stream cache entries and extractor-local font
  bindings between page reads. Parsed fonts, page metadata and recovered
  non-stream objects remain available. Callers serialize extraction and cleanup.
- `extractors/text.rs`, `extractors/text/raw.rs`: original emission order,
  complete Unicode clusters, original character/font identity, normalized
  font advances, actual cursor advances, WMode, glyph frame and origin,
  ascent/descent bounds, rendering visibility and MCID. The raw path adds no
  spaces, performs no sorting or deduplication, retains invisible OCR, respects
  ActualText and nested Form BBoxes, and propagates content parse errors.
  Ordinary extraction APIs retain their existing layout behavior.
- `fonts/unicode_decode.rs`, `fonts/font_dict.rs`: preserve single-byte ASCII
  in mixed-byte CJK encodings; a Differences entry replaces the base encoding
  even when its glyph name is unknown; retain compound glyph-name mappings;
  recognize the TeX `negationslash` glyph as U+0338.
  The existing string-valued glyph-name decoder is exposed for checking
  substitute CFF font coverage without inventing a second glyph-name table.
- `fonts/cmap.rs`, `fonts/cmap/string_ranges.rs`: lazily decode sequential
  UTF-16BE bfranges as complete strings, including multi-codepoint targets and
  ranges beyond the old eager-expansion bound. Later entries override earlier
  ones without expanding the whole range.
- `converters/pdf_to_ir.rs`: initialize the new `DocumentIR::defined_names`
  field required by the resolved office_oxide 0.1.13 dependency.

Modified upstream files carry a `Modified by fastctx` notice. New raw API and
CMap code is independently implemented. No MuPDF source code is included.

Whitespace-only edits also normalize trailing spaces in src/pipeline/input_parsers/{html,markdown,plaintext}.rs and src/rendering/text_rasterizer.rs.
