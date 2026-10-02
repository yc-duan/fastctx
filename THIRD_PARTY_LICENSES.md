# Third-party licenses

The `fastctx` executable embeds the dynamic Pdfium library from
`bblanchon/pdfium-binaries` release `chromium/7763`. The archive and extracted
library are both verified against pinned SHA-256 digests during the build.

The complete notices shipped by that release are preserved under
`third-party/pdfium-7763/`, including the Pdfium BSD license and notices for
Abseil, AGG, fast_float, FreeType, ICU, Little CMS, libjpeg-turbo, OpenJPEG,
libpng, libtiff, LLVM libc, simdutf, and zlib.

Every Rust crate linked into the executable is listed in
[`THIRD_PARTY_LICENSES_RUST.md`](./THIRD_PARTY_LICENSES_RUST.md) together with the
full text of each license that applies. That inventory is generated from
`Cargo.lock` and re-checked against it on every CI run.

The patch parser and context matcher include code derived from OpenAI Codex,
Copyright 2025 OpenAI, licensed under the Apache License 2.0. The source is
https://github.com/openai/codex at revision
`7431f10d0d4b4ecf5df08a853b41859013b17e45`. FastCtx modifies this code for
synchronous, encoding-preserving local writes. Its notice is also retained in
`src/write/patch/NOTICE`; the license text is in `LICENSE-APACHE`.

The project itself is available under the Apache License 2.0.
