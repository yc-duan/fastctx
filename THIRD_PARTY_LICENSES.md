# Third-party licenses

When built with the default `pdf` feature, the `fastctx` executable embeds the
dynamic Pdfium library from `bblanchon/pdfium-binaries` release
`chromium/7763`. The archive and extracted library are both verified against
pinned SHA-256 digests during the build.

For PDF-enabled distributions, the complete notices shipped by that release
are preserved under `third-party/pdfium-7763/`, including the Pdfium BSD
license and notices for Abseil, AGG, fast_float, FreeType, ICU, Little CMS,
libjpeg-turbo, OpenJPEG, libpng, libtiff, LLVM libc, simdutf, and zlib.

The Windows ARM64 community build labelled `no-pdf` is built with
`--no-default-features`; it does not contain Pdfium or the bundled Pdfium
components above.

Rust dependency license metadata is available through `cargo metadata` and the
corresponding crate packages. The project itself is available under either the
MIT License or Apache License 2.0.
