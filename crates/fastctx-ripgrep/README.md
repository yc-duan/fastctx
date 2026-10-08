# Bundled ripgrep

`vendor/manifest.json` pins ripgrep 15.2.0 from BurntSushi/ripgrep. Five images
are unmodified official release executables; their archive digests were
checked against GitHub's asset digests and upstream `.sha256` files. Windows
ARM64 is built from the same unmodified official source as described below.
Each `.gz` contains only `rg` or `rg.exe`, compressed with an empty filename
and zero mtime. The manifest pins every image's size and SHA-256.

The Windows assets are MSVC builds; Linux assets are static musl builds;
macOS assets are native Mach-O builds. Each OS has x86_64 and aarch64 images.
GNU Linux development builds intentionally select the same musl executable.
Unsupported targets fail the build, rather than depending on a system rg.

Cargo verifies and embeds only the target's executable, including in direct
`cargo build` invocations. No downloader is part of the build or the runtime.
`python scripts/ripgrep.py verify` verifies all six local payloads;
`python scripts/ripgrep.py fetch` restores the five release images from the
pinned archives and verifies the vendored Windows ARM64 source build. Restore
that source build from version control, or rebuild it using the recipe below.

The node calls `place` under its existing startup lock, inside the directory
named by fastctx's content fingerprint. Its children therefore keep the same
rg even after the original npm cache or installer directory disappears. The
existing job lifetime rules retain that directory while an old job uses it.
A missing or different image is staged, synced and atomically replaced; an
unreadable image fails startup. A matching image is left in place, including
when a running Windows process has it open.

`COPYING`, `LICENSE-MIT` and `UNLICENSE` are copied from the official archives,
normalizing their Windows CRLF line endings to LF. `PCRE2-LICENCE.md` is from
the pinned PCRE2 10.45 tag, whose URL and digest are in the manifest.
`DEPENDENCIES.md` was generated with cargo-about 0.9.0, the repository's
`about.toml` and `about.hbs`, and ripgrep 15.2.0's Cargo.lock and `pcre2`
feature. The manifest records that source archive's URL and digest. Reproduce
the dependency notice after extracting that verified source archive:

```sh
cargo about generate --manifest-path RIPGREP_SOURCE/Cargo.toml --features pcre2 --locked --fail --config about.toml about.hbs --output-file dependencies.md
```

The vendored notice also lists the crates.io source URLs from that lockfile.
`scripts/notices.py` includes these texts in the existing release notice file;
`scripts/package.py` refuses packages missing either the notices or the exact
embedded executable. The legacy four-member archives and npm relationships
stay the same.

## Windows ARM64 source build

The official 15.2.0 ARM64 executable imports `VCRUNTIME140.dll`. Upstream's
`.cargo/config.toml` enables static CRT only for x86_64 and i686. To run on a
clean ARM64 installation, this one image is built from the pinned official
source archive without changing source files, using static CRT and PCRE2.
It is not an unmodified upstream release binary. The manifest records the
source and Cargo.lock digests and the build tools.

The carried image was cross-compiled on Windows x64 with Rust 1.92.0,
clang/lld 18.1.8, cargo-xwin 0.23.1, Microsoft CRT 14.44.17.14 (35220), and
Windows SDK 10.0.26100. Extract the verified source archive outside a Git
checkout, or set `GIT_CEILING_DIRECTORIES` to its parent so upstream's build
script does not report the enclosing repository's revision. Use absolute
native paths for `XWIN_CACHE_DIR` and `CARGO_TARGET_DIR` on Windows.

```sh
RUSTUP_TOOLCHAIN=1.92.0 PCRE2_SYS_STATIC=1 XWIN_ARCH=aarch64 \
RUSTFLAGS='-C target-feature=+crt-static -C link-arg=/DEBUG:NONE -C link-arg=/Brepro' \
cargo xwin build -j 2 --locked --profile release-lto --features pcre2 \
  --target aarch64-pc-windows-msvc --manifest-path RIPGREP_SOURCE/Cargo.toml \
  --xwin-crt-version 14.44.17.14 --xwin-sdk-version 10.0.26100
```

`/DEBUG:NONE` omits external PDB files, which this executable does not ship;
`/Brepro` avoids a wall-clock PE timestamp. The resulting PE must have ARM64
machine type and no dynamic CRT imports, as `scripts/ripgrep.py verify`
checks. A rebuilt image needs review and a new pinned image digest before
replacing the vendored file. Normal fastctx builds consume the committed
image and never require these cross-compilation tools or download a runtime.
