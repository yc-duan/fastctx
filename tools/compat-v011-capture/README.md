# FastCtx v0.1.1 compatibility capture

This independent Cargo package is the one-time evidence generator for
`tests/compat/v0_1_1`. It never links the current FastCtx crate. Daily CI must run
`../verify-v011-assets.py`; it must not run this package or obtain an old binary.

Every capture arm requires an absolute path to either:

- a clean `cargo build --locked --release` from commit
  `64a6a45f88e65a2c0305e36673fa5e3f99d95384`; or
- the matching binary extracted from the official `v0.1.1` release archive after
  checking `provenance/release-assets.sha256`.

The ceremony runs each case in 32 fresh server processes with a fresh HOME for
every process. Locale and time are fixed to `C.UTF-8`/UTC, Git environment
overrides are removed, and XDG/Windows profile and temporary paths point at that
fresh HOME. The fixture root must be an absent, ASCII, absolute path whose
final component starts with `fastctx-v011`. Its direct parent must contain an
empty, real `.git` directory. This isolates `ignore` crate repository discovery
from the runner's surrounding filesystem while preserving the frozen project
filter semantics.

```text
cargo run --locked --release --manifest-path tools/compat-v011-capture/Cargo.toml -- \
  capture --binary <ABSOLUTE_BINARY> \
  --assets <ABSOLUTE_REPOSITORY>/tests/compat/v0_1_1 \
  --fixture-root <ABSOLUTE_PRIVATE_PARENT>/fastctx-v011-<PLATFORM>-<ORACLE> \
  --platform <windows-x64|linux-x64|macos-x64|macos-arm64> \
  --oracle <source-built|release> --runs 32 --seed 262979601
```

After all eight arms and `provenance/toolchains.json` are present, finalization
accepts only the canonical target order and writes a single common corpus:

```text
cargo run --locked --release --manifest-path tools/compat-v011-capture/Cargo.toml -- \
  finalize --assets <ABSOLUTE_REPOSITORY>/tests/compat/v0_1_1 \
  --platform windows-x64 --platform linux-x64 \
  --platform macos-x64 --platform macos-arm64
python3 tools/verify-v011-assets.py
```

Finalization refuses an incomplete matrix, unstable ledgers, protocol drift, or
cross-arm output disagreement. Never edit `expected.*`, stability ledgers, raw
transcripts, or `manifest.json` by hand to make verification pass.
