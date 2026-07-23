# codex-fastctx

Compatibility package name for
[FastCtx](https://github.com/yc-duan/fastctx) — structured repository tools
for coding agents, Codex first. It contains no binary or install script; its
tiny command shim forwards directly to the `fastctx` dependency, installing
the same `fastctx` command. The shim also identifies this compatibility package
so the TUI checks and updates `codex-fastctx` itself rather than changing the
user’s installation channel.

The forwarded `read` tool can pack 1–32 known text files into one ordered call
with exact per-file continuation parameters. Images, PDFs, and hex view remain
single-file reads.

The forwarded binary includes FastCtx's optional five-tool Bash terminal.
Background jobs survive MCP server and Codex restarts, and can be rediscovered
with `job_list` or managed from `fastctx jobs`. Current-user
`fastshell.job_storage_limit_mib` (default 1024 MiB) and
`fastshell.max_running_jobs` (default 128) control retained finished records
and concurrent jobs. `job_list` defaults to running jobs, with explicit
finished/all views, and `fastshell.job_list_limit` controls its default page
size (20, valid range 1–100). All three settings take effect immediately.
For builds and tests, `job_output` supports `wait_for="exit"` so intermediate
lines accumulate without ending the long poll early.

grep/glob retains automatic CPU parallelism by default. The TUI can set the
machine-level `search.max_cpu_cores` to any integer from 1 through the detected
engine ceiling (available parallelism capped at 16); newly started servers read
it directly without Apply. Invalid values are rejected with the legal range.
The same Config screen offers a default-No confirmation to reset all user
preferences while preserving the Apply receipt, installed binary, host
integration, and running jobs. Normal retention may evict excess finished
records when the history quota returns to its default.

FastCtx-owned non-interactive children never allocate a Windows console window.
The TUI Jobs dashboard aggregates only currently running jobs across FastCtx
instances, groups them by source-session metadata, and shows aligned ids,
width-aware command ellipses, exact start time, and live elapsed time. Finished
output remains agent-readable through `job_output`; MCP hosts do not expose
conversation titles or ids, so the dashboard does not fabricate them.

```console
npx codex-fastctx
```

Prefer installing `fastctx` directly unless you specifically need this name.

If your npm registry is a mirror that has not synchronized this release yet,
the install can fail with `404 Not Found` on the platform package. Add
`--registry=https://registry.npmjs.org/` to that single install command.
