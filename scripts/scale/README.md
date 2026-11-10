# Scale reads

This extends `placement_drill`: a real loopback hub, real node, authenticated device and local IPC clients. It never invokes a model. The Python standard library generates sources; the read driver uses the same conversation, image, collaboration and window APIs as the GUI. First-screen timing ends at window JSON, before WebView painting.

Generate outside the repository on a data disk. Directories must be new. Generation is single-process, low-priority and streaming, followed by an independent complete size/SHA-256/line-count scan. Seed `20261108` is the default. The quick hot conversation's newest 30,000 source rows are identical at 1x and 10x; older history is prepended. Sources contain paired tools, native agents and sidechains, valid base64 PNGs and a pathological tool-result line. Sidechains account for 4/9 of normal transcript bytes. The [versioned manifest](MANIFEST.md) distinguishes source rows and projected events.

```sh
python3 scripts/scale.py generate --root /data/scale/quick-1 --history 1
python3 scripts/scale.py generate --root /data/scale/quick-10 --history 10
python3 scripts/scale.py verify --root /data/scale/quick-1
# Manual: 500 conversations, over ten million projected events,
# a 1–2 GiB source with over 300,000 events, 287 MB single line, 10 GiB output.
python3 scripts/scale.py generate --root /data/scale/full-1 --profile full
```

Quick has 50 conversations and 100,000 / 1,000,000 workload source rows, plus image and agent-link rows. Fixed assets and output do not grow with the history multiplier. Full-10 requires a larger disk and an explicit `--max-gib` increase. Quick-1, quick-10 and full-1 occupy about 29 GB together, before experiment images and binaries. No earlier data is automatically removed.

Build only the kernel and drill using the project's usual shared target and build lock. Keep fixed copies of the binaries for repeated runs. An ordinary checkout's Cargo command is:

```sh
cargo build --locked --release -j 1 -p fastctx -p fastctx-hub --bin fastctx --test placement_drill
DRILL=/absolute/path/to/target/release/deps/placement_drill-HASH
KERNEL=/absolute/path/to/target/release/fastctx
```

Debug builds also work; record optimization explicitly and compare like builds. Calibrate using the actual Claude parser separately from timed actions. `--all` checks every source; without it, the hot source, one child and the long line are checked. This exposes the parser's own memory cost; the generator remains bounded.

```sh
FASTCTX_HOME=/data/scale/calibration-world "$DRILL" /data/scale/quick-1 "$KERNEL" --calibrate-scale --all
```

Run on Linux as root with systemd, loop devices and ext4 tools (WSL works). The launcher creates private network, mount and PID namespaces, enables only loopback, gives every process private homes and a private passwd view, and refuses model executables on PATH. No services are installed. Low priority and two-CPU affinity apply to children. A transient systemd scope limits all experiment processes and their charged page cache to 4 GiB by default (`--memory-gib`), with swap disabled. This is an aggregate experiment limit, not a claim about fitting a node on a 2 GiB server.

`--image-parent` must be on a native Linux filesystem. The launcher creates one temporary ext4 image, copies the fixture and fixed binaries into it, verifies the copy, then runs entirely inside it. The paired command reuses that image sequentially with fresh worlds, so it needs space for the larger case rather than both. The image and runtime worlds are removed when finished. Only evidence remains under the new `--root` directories. On WSL, check the Windows volume containing the distribution's VHD as well as Linux free space. Do not put the image on drvfs or read sources through it during timing.

```sh
sudo python3 scripts/scale.py run --fixture /data/scale/quick-1 --root /data/scale/run-1 \
  --next-fixture /data/scale/quick-10 --next-root /data/scale/run-10 \
  --kernel "$KERNEL" --drill "$DRILL" --image-parent /var/lib/scale-images \
  --quiet-seconds 600 --repeats 1 --memory-gib 4
python3 scripts/scale/summary.py /data/scale/run-1/scale-result.json \
  /data/scale/run-10/scale-result.json --output /data/scale/comparison.json
```

Each run measures images, a native subagent, `who` and sleeping-Claude `offers`, then ten quiet minutes with a connected idle device, then conversation openings and collaboration. Add `--stop-after-first-screen` to keep the same preceding workload and quiet interval, stop after the first valid local window, and skip history completion, page up, device openings and collaboration. It returns nonzero if any preceding operation fails.

The quiet interval has no user, watcher or doctor requests between its boundaries. A failed probe remains a failure; the driver restarts only its own node before attempting the next independent probe. Each conversation opening uses its own client process: publishing a result exits that process, and a deadline kills and reaps it, including detached cache threads. Device openings reuse the same disk cache across new client processes. Restarts and page-process exits are recorded. A killed driver leaves checkpointed evidence, and the namespace coordinator removes any remaining experiment processes before the next case.

Every local opening has a fresh view cache. The first also has a cold node history cache; later ones may reuse it. The device is measured with an empty then a populated view cache. A cached opening does not await renewed host confirmation. Source verification and earlier image reads warm the filesystem cache; these are not cold-disk measurements. First-window arrival, complete-history arrival and the wait for older pages are separate measurements. Real updates feed the shared GUI core before three older-page requests anchored at the same message UUID in both histories. The newest message, event count and page presence are checked. Page hashes can be compared; the entire first window can differ because existing segment boundaries depend on history length.

The operation names and measurement boundaries describe first screen and page up. The current backend adapter is `read_scale/page.rs`; when local pagination replaces it, keep those boundaries and datasets, and replace the adapter. Window/history byte counts are serialized application JSON, not TLS wire bytes. First-screen resource counters start inside the page worker after device connection, at the same open boundary as latency; full-probe counters also include process startup and connection. Counting complete-history JSON adds instrumentation CPU; that elapsed time is reported separately and subtracted from history/page-wait timing, but remains in process CPU counters.

Images are measured as original location/streaming (with hash verification), direct-path thumbnails, host-record thumbnails and device thumbnails with empty/populated caches. Subagent event counts are checked; collaboration query and GUI watch must contain all expected branches. Job metadata uses real serializers; large output is hard-linked into the private world. Skills, output and a binary attachment are scale fixtures, but their replication/transfer is not yet measured.

`scale-result.json` is checkpointed after each operation, and first-window progress before waiting for older history. Failed checks, errors and deadlines remain visible and cause nonzero exit. Ordinary requests have a 180-second deadline; page processes default to 600 seconds (`--page-timeout-seconds`). `processes.jsonl` samples every process in the private namespace each 250 ms, including client, hub and thumbnail workers; a very short-lived process can fall between samples. Node reaped-child CPU is also recorded. `read_chars` includes cached reads and IPC, while `disk_read_bytes` measures physical IO. Doctor readings provide attribution but omit some streamed reads. Cgroup memory peaks and OOM counters distinguish resource exhaustion from request errors.

Run measurements serially, without concurrent builds or generation. Do not drop the machine's global page cache. Report optimization, filesystem, page-cache state and competing load. A few repeats are not p95/p99 claims. These are completed, static Claude conversations; active-host behavior is separate. Codex app-server ingestion, new-device synchronization, WebView rendering and native mobile background behavior remain unmeasured.
