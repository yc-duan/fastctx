# Local kernel releases

No script here publishes a release. Build with Rust 1.92.0. Linux also needs
cargo-zigbuild 0.23.0 and Zig 0.14.1; install the Rust musl target first.

Every fastctx build embeds ripgrep 15.2.0 for its
target. The six compressed images are committed under
`crates/fastctx-ripgrep/vendor`; Cargo decompresses and verifies the selected
image's pinned SHA-256 without a network request. Both GNU and musl Linux
builds carry the official static musl image. macOS and Windows x64 use
their matching official release images. Windows ARM64 is built from the
same unmodified official source with a static CRT: the upstream ARM64
release otherwise requires `VCRUNTIME140.dll`.

`python scripts/ripgrep.py verify` checks all six images, including Linux's
absence of an interpreter and shared dependencies, and Windows's absence
of dynamic CRT imports. `fetch` restores the five upstream release images
from fixed URLs after checking each archive and image against
`vendor/manifest.json`, and verifies the vendored ARM64 source build. The
source build's recipe is in `crates/fastctx-ripgrep/README.md`. These are
maintainer operations, never build or installation steps. Updating ripgrep
requires reviewing the manifest, licenses and six images together.

When a node pins fastctx under `<world>/bin/<fingerprint>/`, it atomically
places `rg[.exe]` beside that copy. A copied executable therefore retains rg
through npm cache deletion, GUI sidecar placement, server installation and
`apply`'s stable copy. No extra Release archive member or npm dependency is
needed. The login shell restores this directory to the front of PATH after
profiles run. Packaging rejects a fastctx without the exact pinned image
and rejects notices that omit ripgrep's license texts.

```sh
rustup target add --toolchain 1.92.0 x86_64-unknown-linux-musl
node scripts/kernel.mjs x86_64-unknown-linux-gnu
node scripts/kernel.mjs aarch64-unknown-linux-gnu
node scripts/kernel.mjs x86_64-pc-windows-msvc
```

Each command prints the actual binary path and build target. Set
`CARGO_TARGET_DIR` and `CARGO_BUILD_JOBS` for the machine's build allocation.
Linux is static musl, with no ELF interpreter or shared library dependencies;
the archive retains the historical `unknown-linux-gnu` name. Windows uses the
static MSVC runtime. `gui/scripts/release.mjs` calls this same builder before
copying the exact kernel into Tauri's sidecar directory. Its installer signing
step is separate and requires the application's configured signing material.

Generate `dist/THIRD_PARTY_LICENSES.md` before invoking the kernel builder or
the GUI release script. The builder embeds that exact file, verifies all three
embedded legal texts, and rejects a notice file from a different lockfile or
supplemental-license inventory. `FASTCTX_THIRD_PARTY_LICENSES` can select another
generated file. A build without it, as an ordinary development build, embeds a
line saying it carries no third-party notices, and packaging rejects it.
`fastctx --licenses` prints the embedded texts without starting a World.
When ripgrep is extracted, the same three readable files are placed beside it.

Generate third-party notices with the matching Rust toolchain and cargo-about
0.9.0, then assemble local packages:

```sh
cargo +1.92.0 run --locked --release -p fastctx-tui --example syntax_notices > syntax-notices.md
RUSTUP_TOOLCHAIN=1.92.0 python scripts/notices.py --syntax-notices syntax-notices.md --output dist/THIRD_PARTY_LICENSES.md
python scripts/package.py roots --notices dist/THIRD_PARTY_LICENSES.md --output dist
python scripts/package.py platform --target x86_64-unknown-linux-gnu --binary TARGET/x86_64-unknown-linux-musl/release/fastctx --notices dist/THIRD_PARTY_LICENSES.md --output dist
python scripts/package.py verify --output dist
```

`package.py` accepts the historical release target. A package contains exactly
the executable, `LICENSE-APACHE`, `NOTICE`, and `THIRD_PARTY_LICENSES.md`, all
regular files at its root. `SHA256SUMS` covers archives. The platform JSON
records both the historical target and actual build target, and each member's
SHA-256. The server command takes the archive digests from `SHA256SUMS`.
All six platforms can be checked together with `verify --complete`.

The GUI's before-build command also generates readable installation resources
with `scripts/gui-notices.py`. It scans the GUI's own locked Rust workspace,
retains its exact-version source archive links, and joins that output with the
kernel, frontend, embedded PDF.js components and font notices. Vite records the
actual input package inventory in `dist/licenses/frontend-packages.json`;
packages without a complete license stop the build. The Tauri resources contain
`LICENSE-APACHE`, `NOTICE`, and `THIRD_PARTY_LICENSES.md` under `licenses/`.
Cargo-about and the Rust toolchain's copyright files must be installed for the GUI too.
On a fresh checkout, fetch the GUI's separate locked graph with
`cargo +1.92.0 fetch --locked --manifest-path gui/src-tauri/Cargo.toml` before
the offline notice step. Its workspace selects the same vendored pdf_oxide
patch as the kernel. Native WebView2 SDK and Rust-binding notices are pinned
separately in `gui/licenses/webview2-sources.json`; updating those crates
requires reviewing that SDK inventory too.

# Inviting a Linux server

The desktop GUI can call `fastctx_install::server::command(Kernel, invitation,
Source)`. `Kernel` supplies a stable version and optional **archive** SHA-256
values for x86_64 and aarch64. The line selects by `uname -m`; a missing digest
produces an architecture-specific explanation before any download. `Source::GitHub` names
the project's versioned release. `Source::Hub` supplies an HTTPS release
directory and the base64 SHA-256 of the certificate's DER SubjectPublicKeyInfo;
`certificate_pin(certificate_der)` computes that value from the issuer's public
certificate. Append no archive name to the directory.
The installer appends `fastctx-<architecture>-unknown-linux-gnu.tar.gz`.
The invitation itself remains an opaque word.

The generated line uses a POSIX shell, uname, date, curl, tar with gzip
support, and sha256sum. Missing curl and permanent download errors stop
with an actionable diagnostic. Interrupted transfers resume with exponential
backoff, bounded by twenty minutes and a finite attempt count; persistent
HTTP errors stop earlier. Partial downloads remain in
`$HOME/.cache/fastctx-install` (under `/var/tmp` when HOME is absent) for a
later attempt. A successful installation removes that version's directory.

The line checks the complete archive SHA-256 before unpacking or executing
anything. Hub downloads use `-k`, as the Mac and Windows lines do: the digest
in the trusted invitation line authenticates the bytes, without requiring
curl's newer public-key pin support. The enrollment code is used only by the
verified program over its own authenticated connection. There is no Python
dependency or downloaded executable bootstrap script.

The Rust subcommand copies its own image atomically into `~/.fastctx/bin`,
retains the three license files, joins, and enables the system service. It
uses the caller's identity and asks for sudo only for service registration;
the service runs as the caller's original UID. `FASTCTX_HOME` can select a
different root. A repeat leaves the installed image and an already running
service alone. Interrupted downloads restart; durable placement, enrollment
and service steps repeat without an enrollment receipt. Joining relies on
net-v3's confirmation of the same World/code/identity after a lost reply.
Service registration adapts units to the running manager, including systemd
219. Older managers use a timer to recover an enabled service after a stop.
Errors retain their actual reason; failed automatic restart authorization
is reported during installation and in the renewal status. Agent hosts, login state and Claude/Codex setup are untouched.

# Hosting the release in your World

The root hub serves releases on its existing TLS port. A Linux hub supplies
its own running executable at its exact package version and architecture.
The first authenticated release query makes one streaming gzip archive with
the same four root members as the release chain: `fastctx` (mode 0755),
`LICENSE-APACHE`, `NOTICE`, and `THIRD_PARTY_LICENSES.md` (mode 0644).
The notices are those embedded in that executable. One compression worker
uses gzip's fast setting and hashes compressed bytes as they are written.
Concurrent queries share this result; downloads never compress or hash it.
The archive and its small manifest occupy unlinked files on the hub's disk
until the process exits, including on a crash. Restarting recomputes the
archive from the running image; replacing the image leaves no old archive.
Windows hubs generate no archive.

Hand-placed archives take precedence independently for each architecture.
Prepare `<hub>/releases/v<version>/` with either or both copied regular
archives and their matching `SHA256SUMS`:

- `fastctx-x86_64-unknown-linux-gnu.tar.gz`
- `fastctx-aarch64-unknown-linux-gnu.tar.gz`
- `SHA256SUMS`

Use the packages built by the release chain above, with their matching
`SHA256SUMS`. Preparing the files in a sibling staging directory and renaming
the directory to `v<version>` makes publication one step. The hub need not
restart. A manifest may also contain the other release platforms. The hub
serves the manual manifest unchanged; when none is placed, the generated
manifest describes its own archive after the first release query.

For example, on the hub machine:

```sh
mkdir -p "$HUB/releases/staging-1.0.0"
cp "$DIST/fastctx-x86_64-unknown-linux-gnu.tar.gz" "$DIST/fastctx-aarch64-unknown-linux-gnu.tar.gz" "$DIST/SHA256SUMS" "$HUB/releases/staging-1.0.0/"
mv "$HUB/releases/staging-1.0.0" "$HUB/releases/v1.0.0"
```

Use a version directory that does not already exist. Publishing a different
development build under an existing version needs an explicit replacement of
that directory; commands already issued retain their previous pinned digest.
Symbolic links, junctions, hard links, special files and directory indexes are
not served. The public routes are only
`/fastctx/releases/v<version>/<one of the three filenames>`. A download
streams the named file without hashing it: the line that fetches it carries
the archive digest and checks it. Every available manual archive must match
its manifest when a member asks for the release, before making an invitation.
Missing architectures are represented by absent digests; malformed manifests,
bad digests and I/O errors still fail the query. File reads, hashing and
compression run independently of WebSocket heartbeats;
the two World WebSocket paths retain their transport and frame semantics.
An archive download has no time limit.

The desktop bridge exposes `server_command` with `hubName`, `accountName`, `name`, and `hours`,
returning a `String` containing the single installation line. TypeScript calls
`serverCommand(hub, account, name, hours)`. The client's equivalent is
`World::server_command(hub, account, name, expires)`; local IPC uses
`Call::ServerCommand` and `Reply::ServerInvitation { command }`. User identity
and the existing invitation authorization remain required.

The node asks `Question::KernelRelease { version, allow_partial: true }` for its own package
version. The root answers `Told::KernelRelease { release, certificate }`:
the version and available archive SHA-256 values, plus the DER certificate actually
loaded by that hub's TLS listener. Before computing the SPKI pin, the node
compares that certificate's complete DER digest with its membership's pinned
root certificate. The download directory uses the first root address in the
membership, even when the node is connected through a relay.

`Ask`/`Told` follow the existing control routing through relays. No new
signature or authorization scheme is introduced. The download itself goes to
the membership's root address and curl checks the root's key on that TLS
connection. Protocol numbers are unchanged. Missing `allow_partial` defaults
to false, which requires both digests or returns the existing error channel.
Complete replies serialize exactly as before. Old hubs ignore the new boolean
and still return complete replies or their existing errors. New readers accept
the old strings as optional digests. The local IPC request and reply are unchanged.

Unavailable architectures, including both on an empty Windows hub, produce a
line that explains the missing architecture and the GitHub/manual-release
alternatives. Invalid manual releases return the existing error channel before
an invitation is issued, with their actual reason and placement directory.

# Drills

`smoke.py` installs a private network on the machine it runs on and walks a
release through it; `update-drill.py` updates an installed application from
one release to the next. Both run in CI.

`concurrency.py` is the concurrency and lifecycle drill
(`specs/world-concurrency.md`): one node in a fresh directory, its Codex
login pointed at `model_mock.py`, so no request goes to any model vendor.
Several conversations at once are spoken to, run commands of the World's
`run`, are deleted while they work, and the node is ended at random moments;
then it checks that nothing was stuck, no lock was taken out of order, every
conversation settled and every deletion left nothing behind.

```sh
python scripts/concurrency.py PROGRAM --codex CODEX --minutes 4 --restarts
```

The node finds Codex on the user's PATH as the system keeps it, not on the
drill's: CI installs `codex` there first. `model_mock.py` runs alone too, for
trying a session by hand against it.


# Old Linux lifecycle gates

The `old-linux` drill uses the kernel artifact already built by CI. It boots
an official CentOS 7.9 image with its own 3.10 kernel, tests the actual hub
installation line, file transfer and MCP `run`/`on`, reboots, renews a local
beta.6 installation to the candidate, and uninstalls both fixtures. Images
and the guest Python runtime have pinned digests and are cached; each run
uses a disposable qcow2 overlay. KVM is probed before use, with TCG available
where nested virtualization is unavailable. Guest serial output and phase
logs remain when a check fails.

Run `python scripts/old-linux.py PROGRAM --cache CACHE --work NEW_DIRECTORY`
on a Linux test host with QEMU, qemu-img, genisoimage and OpenSSH. An optional
`--previous` supplies the baseline locally instead of downloading it from
`GITHUB_REPOSITORY`; supply one of these outside GitHub Actions. The script
installs services only in its guest.

The separate `oldkernel-smoke.py` requires a disposable host with root access:
`sudo python scripts/oldkernel-smoke.py WRAPPER PROGRAM --root NEW_DIRECTORY`.
Its temporary systemd generator wraps only services under that test root,
so the hub and service nodes inherit the same seccomp filter as the caller.
The wrapper verifies its rejected syscalls before exec; smoke also checks
service processes and both local and remote shell children. The generator
is removed even when smoke fails. Do not run service smoke on a personal
installation or a production host.
