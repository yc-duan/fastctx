# Distribution packages

Run `python scripts/package.py --help` from the repository. It constructs
both npm roots from `launcher.js`, using the workspace version and exact
optional dependencies on the five `@fastctx` platform packages. Both roots
provide `fastctx` through `launcher.js`; neither needs installation scripts.

The platform command takes a compiled executable and the combined
`THIRD_PARTY_LICENSES.md`. It puts identical executable bytes in an npm
tarball and a GitHub Release archive. Release archives contain exactly four
flat files: the executable, `LICENSE-APACHE`, `NOTICE`, and
`THIRD_PARTY_LICENSES.md`. `verify --complete` requires every target and
both npm roots, checks archive membership and binary identity, and writes
`SHA256SUMS` for the Release archives.

The workflow builds and assembles artifacts only. It has no npm or GitHub
Release publishing step. When publishing, all five platform packages must
be available at the target version before either root is marked latest;
`fastctx` and `codex-fastctx` are released together. Release users on
0.2.4–0.2.6 can use their in-app updater. Earlier Release versions require
a manual reinstall because their archive contract requires `LICENSE-MIT`.
