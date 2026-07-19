# FastCtx

### Let Codex find the right code sooner.

FastCtx gives coding agents structured repository tools — `read`, `grep`,
`glob`, `replace`, and an optional bash terminal — served by one local Rust
binary over MCP.

The optional terminal publishes `run`, `run_background`, `job_output`,
`job_kill`, and `job_list`. Background jobs have no automatic timeout and
survive MCP server and Codex restarts; their rolling output and exit status
remain addressable by job id under `~/.fastctx/jobs/`.

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
Job commands, working directories, rolling output, and exit status stay in the
current user's private local directory and are never uploaded by FastCtx.

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
and no telemetry. The interactive TUI opens immediately while FastCtx checks
this exact npm package in the background; successful results are cached for 24
hours in machine-private storage. Transient failures stay quiet and remain
available under Status. If GitHub is newer while npm is still propagating, the
UI says so and offers retry. Updates require confirmation, install an exact
version with lifecycle scripts disabled, and restart through a copied helper.
A failed update restores and reopens the previous version with a warning.
`fastctx serve` and MCP tool calls do not perform update traffic; commands run
through the optional bash tools keep their normal network access.

Run `fastctx` in a terminal for the full-screen control UI (preview, Apply,
Jobs, doctor, Unapply), `fastctx jobs` for scriptable running-job management,
or `fastctx serve` for the stdio MCP server. For MCP pipes
the launcher proxies stdio, forwards termination signals and the native exit
code, and closes the Rust server cleanly if the Node parent is killed.

The Jobs dashboard aggregates every currently running job from the current
user's registry across FastCtx server and TUI instances. It groups jobs by
honest source-session metadata, aligns ids and width-aware command ellipses,
and shows exact start plus live elapsed time. Finished output remains available
to the agent through `job_output`; the host does not expose conversation titles
or ids to MCP, so FastCtx does not invent them.

Full documentation: https://github.com/yc-duan/fastctx
