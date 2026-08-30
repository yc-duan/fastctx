# FastCtx

### Structured repository tools for coding agents.

FastCtx is one local Rust MCP runtime for repository inspection, search,
discovery, mechanical replacement, and optional Bash execution. Every
successful text result starts with a compact coordinate line:

```text
=== V:/repo/src/main.rs (lines 120-159 of 512) ===
```

The head note says exactly what the response contains. FastCtx 1.0 has no
trailing `Complete` / `Partial` sentinel, does not repeat continuation
parameters in results, and reads one file per `inspect_local_file` call.

FastCtx publishes nine tools. Any non-empty subset of the four file tools is
valid: `inspect_local_file`, `grep`, `glob`, and `replace`. The five Bash tools
are one atomic group: `run`, `run_background`, `job_output`, `job_kill`, and
`job_list`. New agent connections default to the four file tools.

The control terminal can connect, diagnose, and disconnect nine user-level
agent targets:

- Codex / ChatGPT (`codex`)
- Claude Code (`claude-code`)
- Cursor (`cursor`)
- VS Code Copilot Agent (`vscode-copilot`)
- OpenCode (`opencode`)
- Antigravity (`antigravity`)
- TraeCode CLI (`trae`)
- ZCode (`zcode`)
- Qoder (`qoder`)

```console
npm install --global fastctx
fastctx
```

For a one-off run, use `npx fastctx`. Scriptable examples:

```console
fastctx apply --target cursor --tools inspect_local_file,grep,glob,replace --yes
fastctx doctor --target cursor
fastctx unapply --target cursor --yes
```

Target Disconnect preserves the shared installation, settings, jobs, and other
agent connections. `fastctx unapply --yes` without `--target` performs the
separate full uninstall. Apply and Disconnect are previewed, ownership-aware
transactions; concurrent user edits stop the commit instead of being silently
overwritten.

Background jobs have no automatic timeout and survive MCP server or agent
restarts. Output, status, and a directly readable plain-text log are retained
under `~/.fastctx/jobs/` up to the job's frozen storage ceiling. `job_output`
returns when the job ends or `wait_ms` elapses and identifies the exact stored
line ranges it delivers. Running background state may accompany later text
responses; a terminal transition is acknowledged only after the full or compact
readout actually fits in a response. This readout refreshes on tool calls and is
not a push notification.

On Windows, FastCtx-owned non-interactive children do not allocate console
windows. grep/glob automatic parallelism is bounded by the detected engine
ceiling and can be configured explicitly. Current-user background storage,
concurrency, page size, replace size, output budgets, and update behavior are
managed from the TUI.

This launcher selects the matching scoped package locally: Windows x64,
Windows arm64, Linux x64, macOS x64, or macOS arm64. It has no postinstall
script and no telemetry. The bounded startup update check only runs for the
interactive TUI; `fastctx serve` and MCP calls perform no update traffic.

Use `fastctx` for the full-screen control UI, `fastctx jobs` for scriptable
running-job management, or `fastctx serve --tools <csv>` for a direct stdio MCP
server.

Full documentation: https://github.com/yc-duan/fastctx
