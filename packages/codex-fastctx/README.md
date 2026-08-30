# codex-fastctx

Compatibility package name for
[FastCtx](https://github.com/yc-duan/fastctx), a local Rust MCP runtime with
structured repository tools for coding agents. This package contains no binary
or install script. Its small command shim forwards to the exact `fastctx`
dependency and preserves `codex-fastctx` as the installation/update channel.

```console
npx codex-fastctx
```

Prefer installing `fastctx` directly unless you specifically need this package
name. If this compatibility name owns an existing 0.x command, upgrade it with
`npm install --global codex-fastctx@1.0.0`; do not co-install both root package
names in one global prefix because each exports the same `fastctx` shim.

FastCtx 1.0 publishes `inspect_local_file`, `grep`, `glob`, `replace`, and an
optional atomic five-tool Bash group. File tools may be selected in any non-empty
combination for each agent. Successful text responses start with a compact
`=== subject (metric; facts) ===` coordinate line; there is no trailing
`Complete` / `Partial` sentinel, continuation-parameter echo, or multi-file
inspect request.

The forwarded control terminal can connect and diagnose Codex / ChatGPT, Claude
Code, Cursor, VS Code Copilot Agent, OpenCode, Antigravity, TraeCode CLI, ZCode,
and Qoder. Target Disconnect removes only the selected agent integration; full
Unapply remains a separate operation.

Background jobs survive MCP server and agent restarts. Their retained output,
status, and direct log path remain addressable by job id under
`~/.fastctx/jobs/` up to the job's frozen storage ceiling. `job_output` reports
the exact stored line ranges it returns. Running state can accompany later tool
results; one terminal transition is acknowledged only after a full or compact
readout actually fits in a response. This is refreshed by tool calls, not pushed.

FastCtx-owned non-interactive children do not allocate Windows console windows.
The launcher contains no telemetry, and MCP tool calls perform no update traffic.

If a mirror registry has not synchronized the release, add
`--registry=https://registry.npmjs.org/` to that single install command.
