# FastCtx

### Let Codex find the right code sooner.

FastCtx gives coding agents structured repository tools — `read`, `grep`,
`glob`, `replace`, and an optional bash terminal — served by one local Rust
binary over MCP. `read` can pack 1–32 known text files into one request-ordered
call with exact per-file continuation parameters, while images, PDFs, and hex
view remain single-file reads.

The optional terminal publishes `run`, `run_background`, `job_output`,
`job_kill`, and `job_list`. Background jobs have no automatic timeout and
survive MCP server and Codex restarts. Current-format jobs keep their complete
output log and exit status addressable by job id under `~/.fastctx/jobs/`.
`job_output` is a query with a caller-chosen delay: it returns when the job
ends or when `wait_ms` elapses, showing the newest output not yet seen, and
names the log path and exact line numbers for anything it leaves out. Records
from the preceding segmented format remain readable but do not claim direct
log coordinates or recover output their original rolling window evicted.

On Windows, all FastCtx-owned non-interactive children run without allocating
a console window; no caller flag is required. Commands that explicitly launch
a GUI or another terminal keep that visible behavior.

Finished records are retained until the current user's
`fastshell.job_storage_limit_mib` limit evicts the oldest (default 1024 MiB);
`fastshell.max_running_jobs` caps concurrent jobs across all FastCtx sessions
for that user (default 128). `job_list` shows running jobs by default; explicit
`status="finished"` exposes retained history, while `fastshell.job_list_limit`
sets the default page size (20, valid range 1–100). All three settings take
effect immediately when saved.
Job commands, working directories, output logs, and exit status stay in the
current user's private local directory and are never uploaded by FastCtx.

grep/glob keeps its existing automatic CPU parallelism by default. The TUI can
set an explicit `search.max_cpu_cores` value from 1 through the engine-visible
ceiling (available parallelism capped at 16); each newly started server reads it
directly, without Apply or a copied environment key. Invalid values fail with a
repairable diagnostic instead of being clamped or replaced. The Config screen
also provides a default-No confirmation that resets every user preference while
preserving the Apply ownership receipt, installed binary, host integration, and
running jobs. Restoring the default history quota can evict excess finished
records through the normal retention policy.

```console
npm install --global fastctx
fastctx
```

For a one-off run without installing, `npx fastctx` opens the same control
terminal.

If your npm registry is a mirror that has not synchronized this release yet,
the install can fail with `404 Not Found` on the platform package. Install once
from the official registry:

```console
npm install --global fastctx --registry=https://registry.npmjs.org/
```

This package is the launcher: it selects the matching scoped platform package
(`@fastctx/win32-x64`, `@fastctx/linux-x64`, or the corresponding macOS
package) locally and starts the complete binary. There is no postinstall script
and no telemetry. The interactive TUI checks this exact
npm package for updates before the main menu opens; the wait is strictly
bounded, and a failed or timed-out check enters silently. When a newer version
is installable, the update screen opens directly and asks whether to update or
continue. Successful results are cached for 24 hours in machine-private
storage. Transient failures stay quiet and remain available under Status. If
GitHub is newer while npm is still propagating, the Update screen says so and
offers retry. Updates require confirmation, install an exact
version with lifecycle scripts disabled, and restart through a copied helper.
A failed update restores and reopens the previous version with a warning.
`fastctx serve` and MCP tool calls do not perform update traffic; commands run
through the optional bash tools keep their normal network access.

Run `fastctx` in a terminal for the full-screen control UI (configuration,
reset, preview, Apply, Jobs, doctor, Unapply), `fastctx jobs` for scriptable
running-job management, or `fastctx serve` for the stdio MCP server. For MCP pipes
the launcher proxies stdio, forwards termination signals and the native exit
code, and closes the Rust server cleanly if the Node parent is killed.

The Jobs dashboard aggregates every currently running job from the current
user's registry across FastCtx server and TUI instances. It groups jobs by
honest source-session metadata, aligns ids and width-aware command ellipses,
and shows exact start plus live elapsed time. Finished output remains available
to the agent through `job_output`; the host does not expose conversation titles
or ids to MCP, so FastCtx does not invent them.

Full documentation: https://github.com/yc-duan/fastctx
