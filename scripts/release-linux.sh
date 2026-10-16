#!/bin/bash
# Native Linux release: Rust 1.92.0, Node 22, Python 3.11+, cargo-about 0.9.0,
# cargo-zigbuild 0.23.0, Zig 0.14.1, qemu-user for ARM64 checks, and Tauri's
# Ubuntu build prerequisites. cargo-about requires --features cli.
# The caller supplies its resource-bounded filesystem and process scope.
# A tree without git passes the commit's time as SOURCE_DATE_EPOCH: it orders
# builds of the same version (crates/fastctx-install/build.rs).
# FASTCTX_RELEASE_GUI=0 builds the CLI and server archives only, without
# World's Linux packages.
set -euo pipefail
if [ "$(uname -m)" != x86_64 ]; then
  printf 'This release script builds the x86_64 GUI and both Linux CLI architectures; run it on x86_64 Linux.\n' >&2
  exit 1
fi
gui=${FASTCTX_RELEASE_GUI:-1}
repo=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cd "$repo"
# FASTCTX_RELEASE_JOBS sizes the compilers to the machine; the caller bounds memory and processes.
export CARGO_BUILD_JOBS=${FASTCTX_RELEASE_JOBS:-4} CARGO_INCREMENTAL=0 RUSTUP_TOOLCHAIN=1.92.0
export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$repo/target}
mkdir -p dist
mkdir -p "$CARGO_TARGET_DIR/launcher-check"
node scripts/check-launcher.cjs "$CARGO_TARGET_DIR/launcher-check"
python scripts/ripgrep.py verify
cargo +1.92.0 fetch --locked
if [ "$gui" != 0 ]; then
  cargo +1.92.0 fetch --locked --manifest-path "$repo/gui/src-tauri/Cargo.toml"
fi
cargo +1.92.0 run --locked --release -j "$CARGO_BUILD_JOBS" -p fastctx-tui --example syntax_notices > dist/syntax-notices.md
python scripts/notices.py --syntax-notices dist/syntax-notices.md --output dist/THIRD_PARTY_LICENSES.md
python scripts/package.py roots --notices dist/THIRD_PARTY_LICENSES.md --output dist
export FASTCTX_THIRD_PARTY_LICENSES="$repo/dist/THIRD_PARTY_LICENSES.md"
for target in x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu; do
  node scripts/kernel.mjs "$target"
  if [ "$target" = x86_64-unknown-linux-gnu ]; then
    cargo +1.92.0 test --locked -j "$CARGO_BUILD_JOBS" -p fastctx-ripgrep --target "$target"
    export FASTCTX_TEST_BASH=/bin/bash
    export FASTCTX_TEST_ROOT="$CARGO_TARGET_DIR/lifecycle-check"
    mkdir -p "$FASTCTX_TEST_ROOT"
    for contract in own_lifetime job_lifecycle; do
      if [ "$contract" = own_lifetime ]; then package=fastctx-node; else package=fastctx-hands; fi
      # The test executable this build made, as cargo reports it: a reused target directory keeps
      # the ones earlier builds made beside it.
      mapfile -t tests < <(cargo +1.92.0 zigbuild --locked --release -j "$CARGO_BUILD_JOBS" -p "$package" --test "$contract" --target x86_64-unknown-linux-musl --message-format=json-render-diagnostics |
        python -c 'import json, sys; [print(m["executable"]) for m in map(json.loads, sys.stdin) if m.get("reason") == "compiler-artifact" and m.get("executable") and m["target"]["name"] == sys.argv[1]]' "$contract")
      test "${#tests[@]}" = 1
      "${tests[0]}"
    done
  fi
  binary="$CARGO_TARGET_DIR/${target/-gnu/-musl}/release/fastctx"
  python scripts/package.py platform --target "$target" --binary "$binary" --notices dist/THIRD_PARTY_LICENSES.md --output dist
done
python scripts/package.py verify --output dist
if [ "$gui" = 0 ]; then
  exit 0
fi
cd "$repo/gui"
npm ci
npm run check:wire
node scripts/release.mjs
python "$repo/scripts/linux-bundle.py" --target-dir "$CARGO_TARGET_DIR" --output "$repo/dist/gui"
