# FastCtx

**English** | [简体中文](./README.zh-CN.md)

### Fast, context-efficient repository tools for coding agents.

FastCtx is a local Rust tool runtime. It provides file reading, content search, file discovery, batch replacement, and Bash command execution through MCP.

Repository operations run in a persistent process with stable input schemas and output formats. The model can gather the context it needs in fewer steps and spend more attention on understanding code, planning changes, and verifying results.

Each `fastctx serve` process is a thin stdio proxy. Proxies for the same user and FastCtx build share one private local control center, including its search executor and global admission limits, while every MCP connection keeps its own working directory, native environment, cancellation state, and background-output cursor.

An MCP session ends when its host ends it, never because the shared runtime had a problem. If the control center becomes unreachable, the proxy answers the calls it can no longer complete with an explicit error, reconnects to a replacement — starting one, or running the engine inside the proxy itself — and carries on over the same stdio transport. Side-effecting calls are never replayed. The control center itself stays resident while any host process that used it is still running, and exits ten minutes after the last of them is gone, with no connection, no active request, and no running background job.

```console
npm install --global fastctx
fastctx
```

The `fastctx` command opens the control terminal. Open **Agent connections**, choose an agent, select exactly the tools it should receive, review the proposed changes, and apply them. Restart that agent after the connection succeeds.

FastCtx provides first-class setup for Codex / ChatGPT, Claude Code, Cursor, VS Code Copilot Agent, OpenCode, Antigravity, TraeCode CLI, ZCode, and Qoder. Any other MCP client can register `fastctx serve --tools <csv>` directly.

## What FastCtx solves

Coding agents often assemble shell commands on the fly when they access a repository. They have to handle quotes, escaping, paths, and platform differences, then extract the useful information from terminal output. A simple file read or symbol search can take several tool calls just to confirm that the command is correct and the result is complete.

This work consumes context and reasoning. The model tracks the code problem and the tool mechanics at the same time: whether the PowerShell syntax is correct, whether a path was escaped correctly, whether the encoding produced mojibake, and whether the host truncated a long result. More tool overhead leaves less room for the repository itself.

FastCtx turns common repository operations into structured input and output. The model provides parameters such as a path, pattern, range, and mode. The Rust runtime handles command construction, directory traversal, encoding, pagination, and output boundaries.

The tools cover the main parts of a coding task:

- `inspect_local_file` reads text, images, PDFs, and raw bytes;
- `grep` searches file contents;
- `glob` finds files;
- `replace` performs mechanical batch replacement;
- `run`, `run_background`, `job_output`, `job_kill`, and `job_list` execute Bash commands and manage persistent long-running jobs.

This greatly reduces the attention the model spends on tool mechanics, such as checking whether a PowerShell command is correct. It improves context efficiency and helps tasks finish faster with better results.

## Installation

### Install with npm

Requires Node.js 18 or later:

```console
npm install --global fastctx
fastctx
```

The first launch opens the full-screen control terminal. The interface supports 17 languages and provides these main actions:

1. Connect and diagnose nine supported coding-agent targets from **Agent connections**;
2. Select any non-empty subset of the four file tools, plus the five Bash tools as one atomic group;
3. Adjust the output tier and provider-aware output protection;
4. Keep grep/glob on automatic CPU parallelism or set an explicit core limit;
5. Set current-user background-job storage, concurrency, and AI list page limits;
6. Inspect every currently running job across FastCtx sessions, follow its output, and stop it on the **Jobs** screen;
7. Reset all user preferences to factory defaults through a confirmation screen.

Connecting copies the current binary to `~/.fastctx/bin/` and points the host configuration at that stable path. The connected setup keeps working after npm cache cleanup or upgrades.

On launch, FastCtx checks its launch channel for updates before the main menu opens. A brief checking screen appears and the wait is strictly bounded: if the check cannot finish — offline, timeout, rate limiting — FastCtx enters silently, and the dedicated **Update** screen still offers a manual check at any time. When a newer version is installable, the update screen opens directly and asks whether to **Update & restart** or **Continue** into the current version. Successful results are cached for 24 hours in machine-private storage outside `~/.fastctx`, so most launches skip the network entirely. npm launches query the exact launcher package through a fresh isolated cache with `--prefer-online`; direct GitHub Release executables read the stable tag from GitHub's `releases/latest` web redirect.

If GitHub has published a release but npm has not exposed the matching version yet, FastCtx shows a propagation screen instead of trusting stale metadata. **Retry** always uses another isolated cache; it never clears or mutates the user's normal npm cache. Transient network or rate-limit failures stay quiet and are recorded under **Status**; malformed publication metadata produces one warning. Status also offers a manual check that bypasses the 24-hour TTL. An accepted npm update installs the exact version with lifecycle scripts disabled. A GitHub Release update downloads this repository's platform archive and aggregate `SHA256SUMS`, verifies the archive before safely extracting the binary, probes the downloaded version, replaces the executable atomically, and rolls back when restart health fails. A failed npm update restores the exact previous package version; every failed update reopens the previous TUI with a warning. After a successful restart, the owned `~/.fastctx/bin/` copy is synchronized; externally changed copies are left untouched. Restart Codex after an update so existing sessions and their build-isolated control center are replaced by the new build.

`cargo install` builds and the internal `~/.fastctx/bin/` runtime are not self-updated. Set `FASTCTX_DISABLE_UPDATE_CHECK=1` to disable the TUI startup check.

**Removal** stops FastCtx process images running from the managed bin directory, removes the configuration managed by FastCtx, and deletes its managed data. Shared settings changed by the user after connecting are preserved.

### Upgrading from 0.x

Upgrade the same npm package that owns your command. Use `npm install --global fastctx@1.0.0` for a `fastctx` installation, or `npm install --global codex-fastctx@1.0.0` if you deliberately installed the compatibility name. Do not install both root packages into one global prefix: both export the `fastctx` shim, so their launcher ownership is intentionally not a coexistence contract. The 0.x settings and update-request schemas are migrated in place; a failed global npm update reinstalls the exact previous package version.

For a GitHub Release installation, manually downloading the 1.0 archive and replacing or launching the binary works from every 0.x version. The in-app copied-helper updater can move directly from v0.2.4, v0.2.5, or v0.2.6 to 1.0. Helpers in v0.2.3 and earlier require a mutually incompatible historical archive file set, so use the manual download or npm route for those builds; FastCtx does not present that impossible static-asset transition as automatic.

Codex is the only 0.x agent connection that is migrated. Run `fastctx doctor --target codex`, then `fastctx apply --target codex --yes` to record the 1.0 tool set and refresh an unchanged known legacy guidance block. User-edited or same-name unowned configuration stops with a repair instruction instead of being overwritten. Apply also retires `mcp__fastctx` from `features.code_mode.direct_only_tool_namespaces`, which 0.x added to pin FastCtx to Codex's direct tool surface. 1.0 hands orchestration back to the agent, so that pin is no longer wanted, and `[features.code_mode]` is host-wide tool routing rather than part of registering one server. Your other entries there are untouched, and the removal is listed in the Apply preview. Restart Codex after Apply so every session and the shared control center use 1.0.

### If the install returns 404

Mirror registries copy new releases from the official registry on a delay. Right after a release, an install through a mirror can fail with `404 Not Found` — most often on the platform package, which npm installs as an optional dependency and skips silently, leaving `fastctx` installed but unable to start.

Install once from the official registry:

```console
npm install --global fastctx --registry=https://registry.npmjs.org/
```

The flag applies to this command only and leaves the npm configuration unchanged. To use the official registry permanently:

```console
npm config set registry https://registry.npmjs.org/ --location=user
```

After installation, the **Update** screen probes the npm registry configured on this machine, the official registry, and registry.npmmirror.com, then installs from the first source that carries both the launcher and the matching platform package. Version numbers always come from the official registry and GitHub, so a mirror can never announce a version the official source has not published.

### One-off run

```console
npx fastctx
```

`npx` opens the same control terminal without a global installation. Connecting still copies the binary to `~/.fastctx/bin/`, so the connected setup keeps working after the npx cache is cleaned; only the `fastctx` command itself requires the global installation.

### Non-interactive use

```console
fastctx apply --tier standard --yes
fastctx apply --target cursor --tools inspect_local_file,grep,glob,replace --yes
fastctx status
fastctx doctor --target cursor
fastctx jobs
fastctx jobs kill j-a1b2c3
fastctx unapply --target cursor --yes
fastctx unapply --yes
```

- `apply`: install FastCtx and connect one agent; `--target` defaults to `codex`;
- `status` / `doctor`: check the shared installation and either every connected target or one explicit `--target`;
- `jobs`: list running background jobs;
- `jobs kill <job_id>`: stop one background job and its full process tree;
- `unapply --target <id>`: disconnect one agent and preserve the shared installation;
- `unapply` without a target: remove every FastCtx integration and its managed data;
- `lang <code>`: set the control terminal language.

`status` uses three states: `[PASS]`, `[INFO]`, and `[FAIL]`. It also reports the detected search CPU ceiling and the configured/effective parallelism. A `[FAIL]` result returns a non-zero exit code.

Supported target ids are `codex`, `claude-code`, `cursor`, `vscode-copilot`, `opencode`, `antigravity`, `trae`, `zcode`, and `qoder`. Apply and disconnect are target-scoped and transactional: the preview shows every file change, concurrent edits stop the commit, and a failed write rolls back changes already made. Full Unapply remains a separate operation because it also removes the shared installation and managed data.

### Tool limits and settings reset

grep/glob uses automatic parallelism by default: the operating system's available parallelism, capped at 16. In **Config → Search**, choose a preset with ←/→ or press Enter and type `auto` or any integer in the displayed `1..=maximum` range. The setting is loaded when the shared control center starts and takes effect after that control center restarts, which happens once every Codex process using it has exited. Reconnecting is not required.

The same setting can be written manually in `~/.fastctx/config.toml`:

```toml
[search]
max_cpu_cores = 4
```

Omitting the key keeps the previous automatic behavior. Invalid types, empty values, zero, negative numbers, and values above the engine's displayed ceiling prevent a session from starting and produce a diagnostic without rewriting the file. The limit sets one request's effective search parallelism to its base lane plus shared extra workers, at most N. Across every session in the per-user control center, concurrent requests retain independent base lanes but share one pool of at most N−1 extra lanes, so the upper bound for R concurrent requests is R+N−1. This is not CPU affinity or a strict system-wide governor.

replace accepts files and replacement results up to 256 MiB by default. In **Config → Editing**, choose a coarse 64 MiB–4 GiB preset with ←/→. Saving takes effect on the next replace request, including requests from an already-open Codex session, and does not require reconnecting. Larger limits allow replace to use more memory; values set too high may cause an out-of-memory failure.

The same limit can be written manually; 64 MiB is the minimum and 4096 MiB is the maximum:

```toml
[replace]
max_file_size_mib = 512
```

**Config → Reset → Reset all settings** opens with **No** selected. Confirming restores every user preference, including language, output budgets, Bash/job limits, search CPU limit, replace file limit, and update settings. It preserves the connection receipt, installed binary, host configuration, and running jobs. Restoring the default 1024 MiB job-history quota may immediately evict the oldest finished records above that quota.

### Other distribution channels

```console
cargo install fastctx --locked
```

GitHub Releases provides zip archives for Windows x64 and Windows arm64, and executable-preserving tar.gz archives for Linux x64, macOS x64, and macOS arm64. Every archive includes the binary and complete combined license notices; verify it with the release's aggregate `SHA256SUMS`.

## Tools

FastCtx provides nine MCP tools:

| Tool | Purpose |
|---|---|
| `inspect_local_file` | Read one text, image, PDF, or binary file |
| `grep` | Search contents in a file or repository tree |
| `glob` | Find files by path pattern |
| `replace` | Apply mechanical batch replacements to files or a repository tree |
| `run` | Run a Bash command in the foreground |
| `run_background` | Start a background Bash job |
| `job_output` | Query a background job and show its newest unseen output |
| `job_kill` | Stop the full process tree of a background job |
| `job_list` | Rediscover running and retained finished jobs |

Each agent stores its own enabled set. Any non-empty combination of `inspect_local_file`, `grep`, `glob`, and `replace` is valid. The five Bash tools are atomic: selecting any one publishes all five, and clearing Bash removes all five. A new connection defaults to the four file tools. All selected tools share the `mcp__fastctx` namespace; how a host spells an individual tool inside that namespace is the host's own convention.

### `inspect_local_file`

`inspect_local_file` handles one file per call, returns 1-based line numbers for text, and supports paging:

```json
{
  "file_path": "V:/repo/src/main.rs",
  "offset": 120,
  "limit": 40
}
```

```text
=== V:/repo/src/main.rs (lines 120-159 of 512) ===
120	fn main() {
121	    ...
159	}
```

Every successful text result starts with one `=== subject (metric; facts) ===` head note. The head records what the response actually contains; there is no trailing `Complete`/`Partial` sentinel and no repeated call syntax. Here, the next page begins at line 160, so continue with `offset: 160`. Call known files independently; the former multi-file `files` request shape is not part of the 1.0 contract.

`inspect_local_file` also supports:

- PNG, JPG, GIF, WebP, and BMP images;
- PDF text layers and rendered page images;
- a paged hex view for any file;
- UTF-8, BOM-based encodings, and common legacy encodings.

Automatic encoding detection accepts results with sufficient evidence. When the encoding is ambiguous, the error lists candidates and retry options. Pass `encoding` to select one explicitly:

```json
{
  "file_path": "V:/repo/docs/legacy.txt",
  "encoding": "gbk"
}
```

Use the hex view for binary files:

```json
{
  "file_path": "V:/repo/data/cache.bin",
  "view": "hex"
}
```

### `grep`

`grep` uses the Rust regex engine from the ripgrep family:

```json
{
  "pattern": "fn \\w+_lock",
  "path": "V:/repo/src",
  "output_mode": "content",
  "context": 1
}
```

```text
=== grep "fn \\w+_lock" (matches 1 of 1) ===
V:/repo/src/edit/locks.rs
62-/// Cross-process lock keyed by file identity.
63:pub fn acquire_path_lock(identity: &PathIdentity) -> LockGuard {
64-    ...
```

`output_mode` has four values:

- `files_with_matches`: return matching files;
- `content`: show matches grouped by file;
- `count`: return the occurrence count for each file;
- `summary`: scan the full target and return global totals.

`grep` respects `.gitignore` and `.ignore` by default, includes hidden files, and excludes `.git` and binary files. Common filters include `glob`, `type`, `case_insensitive`, `multiline`, and `context`. Page through results with `head_limit` and `offset`.

Files with uncertain encodings appear in a skip report with their path, reason, and resolution parameters. Use `encoding` for a single file and `fallback_encoding` for a directory search.

If a file changes during a directory search, `grep` reports that file as skipped and continues; a changing single-file target returns an error so partial matches never masquerade as complete results.

A directory the walk cannot enter — denied permissions, a locked file, a symlink loop — never discards the results found around it. `grep`, `glob`, and `replace` return what they reached, list each unreachable path with its cause, and count them in the head note. An unreadable search root is still an error, because a walk that reached nothing cannot report that it found nothing.

### `glob`

`glob` finds files with a pattern relative to the search root:

```json
{
  "pattern": "**/*.toml",
  "path": "V:/repo",
  "sort": "modified",
  "output_mode": "details"
}
```

```text
=== glob (files 1-2 of 2) ===
{"path":"V:/repo/crates/core/Cargo.toml","bytes":1842,"modified":"2026-08-23T16:42:18.123456700Z"}
{"path":"V:/repo/Cargo.toml","bytes":2937,"modified":"2026-08-23T15:07:03.000000000Z"}
```

Main parameters:

- `filter_mode: "ignore"` (default): respect plain `.ignore` files only;
- `filter_mode: "all"`: disable plain `.ignore` filtering;
- `output_mode: "paths"` (default): return one absolute path per line;
- `output_mode: "details"`: return one compact JSON object per line with path, byte size, and a fixed nine-digit RFC 3339 UTC modification time;
- `sort: "path"`: use a stable path order;
- `sort: "modified"`: order files from newest to oldest;
- `offset` / `limit`: page through the result set.

`glob` never reads `.gitignore`, `.git/info/exclude`, or the user's global Git ignore, and it never hides `.git` automatically. Hidden and Git-internal files remain ordinary candidates; exclude unwanted trees explicitly with a negative pattern such as `!target/**` or `!.git/**`. The legacy value `filter_mode: "project"` is still accepted and behaves as `"ignore"`, but is no longer published in the tool schema. `grep` and `replace` keep their existing Git-ignore behavior.

`grep` and `glob` render filename components that are unsafe to place directly in a line as reversible `~fastctx~b...~` or `~fastctx~w...~` escapes. Copy the whole component exactly into a later grep/glob call; do not decode or edit it.

### `replace`

`replace` handles mechanical, deterministic batch changes such as symbol renames, import rewrites, configuration key migrations, and fixed-pattern deletion. Generated code changes and per-location semantic edits are handled by the host's `apply_patch` tool.

```json
{
  "pattern": "old_name\\(",
  "replacement": "new_name(",
  "path": "V:/repo/src",
  "glob": ["**/*.rs"],
  "dry_run": true
}
```

```text
=== replace dry run (12 matches in 3 files; nothing written) ===
...
```

`replace` freezes the candidate set and counts every match before the first write. Use `dry_run` for preview and `max_replacements` to cap the change scope.

Each file is checked again before commit. Writes use atomic replacement in the same directory and preserve the original encoding, BOM, line endings, trailing newline, Unix mode, and untouched bytes. Concurrent changes move the affected file into the failure report while the remaining files continue.

### `run`

`run` executes a Bash command in the foreground and returns merged stdout, stderr, and the exit code. It uses Git Bash on Windows and the system Bash on macOS and Linux.

```json
{
  "command": "cargo test --quiet 2>&1 | tail -n 40",
  "timeout_ms": 180000
}
```

Commands run in a non-interactive environment. Installation, confirmation, and editor commands need flags such as `-y` and `--no-edit`. Non-zero exit codes are returned as execution results.

On Windows, every FastCtx-owned non-interactive child process is created without allocating a console window, including Bash discovery, foreground/background Bash, detached supervisors, and doctor probes. There is no hidden-window parameter to remember. A command that explicitly launches a GUI or a new terminal still has that visible effect.

Output uses bounded memory. The head note reports the exact line ranges present in the response and whether older captured lines were dropped. For commands whose complete output matters, redirect to a known file in the original command and then inspect or search that file; do not rerun a side-effecting command merely to recover omitted output.

#### Command environment

A stdio MCP server does not receive the environment its user configured. The host clears the child environment and re-adds only a fixed core list of names, so variables such as `JAVA_HOME`, `GOPATH`, or `CUDA_PATH` never reach FastCtx and would otherwise never reach the commands it runs.

On Windows, FastCtx restores the environment the operating system persists for the user — the system and user entries of the Windows **Environment Variables** dialog — and lays whatever the host did provide on top of it, so host values always win. `PATH` is the single exception and is a union: the search path that arrived stays exactly as it is, and only persisted directories it does not already contain are appended after it. On macOS and Linux the login shell already sources the profile where a user's environment lives, so nothing is reconstructed.

`run` and `run_background` use a login shell (`bash -lc`) by default so profile-managed toolchains such as nvm, pyenv, and rustup resolve; pass `login_shell: false` for a clean `--noprofile --norc` shell. On Windows a login shell is given the complete Windows search path unless `MSYS2_PATH_TYPE` is already set, in which case that choice is respected.

Two environment variables configure this. Both are read from either the persisted environment or the `env` table of the FastCtx entry in the host's MCP server configuration:

| Variable | Effect |
| --- | --- |
| `FASTCTX_INHERIT_ENVIRONMENT=0` | Skip the restore, leaving commands with the environment the host provided. |
| `FASTCTX_BASH` | Absolute path to the Bash to use. FastCtx requires GNU Bash and never accepts the `System32\bash.exe` WSL launcher. |

### `run_background`

`run_background` starts a background Bash job and returns a job id immediately. It is useful for builds, tests, development servers, and other long-running commands.

Each job is owned by a detached supervisor rather than by the MCP server. It keeps running across server exits, ChatGPT / Codex restarts, and session changes until the command exits or `job_kill` stops it. There is no background timeout parameter.

Output and exit status are stored under `~/.fastctx/jobs/`, so another FastCtx session can resume the same job by id. For jobs started by the current format, output is appended to a plain log file whose path is returned when the job starts, so `inspect_local_file` and `grep` work on the retained prefix directly. At supervisor startup, each job freezes a hard ceiling for the combined log and line index from the current `fastshell.job_storage_limit_mib` setting. If output reaches that ceiling, FastCtx keeps draining the child process so the command can finish, stops persisting further bytes, and records an explicit truncation notice without changing the command's exit code.

While one MCP session has jobs that it started or queried, successful text results can carry a one-line background readout immediately after the head note. Running state may repeat. A terminal transition is shown once and acknowledged only when the full or compact readout actually fits in a response; if the line is omitted for budget or content-channel reasons, it remains pending. The readout refreshes only when another tool is called—it is not a push notification.

### `job_output`

`job_output` queries a background job, including jobs started in earlier sessions, and reports `running`, `exited`, or `interrupted` together with the newest output the caller has not been shown. `wait_ms` (0–240000, default 30000) is how long the query may take: it returns as soon as the job ends and otherwise waits the window out; intermediate lines do not end the wait. Pass `wait_ms=0` for an immediate snapshot, and raise it only when there is nothing else to do because the call blocks. Long current-format output is windowed — the newest lines that fit, plus the start of the log on the first call — and a note names the exact lines that were skipped and the log path to read them from. Line numbers in that log are the same `seq` numbers `after_seq` takes, so moving between the two tools needs no translation. Records written by the preceding segmented format remain readable, including while an older supervisor is still appending, but they do not advertise direct log coordinates and cannot recover bytes that their original rolling window already evicted.

The head note always states the current lifecycle (`running`, `exited`, `killed`, or `interrupted`), the line ranges delivered, and the log path when one exists. There is no trailing status word. Before the per-job disk ceiling is reached, anything a response leaves out is still one `inspect_local_file` or `grep` away. After the ceiling is reached, `job_output` and the Jobs dashboard identify the last stored line and explain that the supervisor continued draining without persistence. The compatibility limitation above applies only to records created by the preceding format.

### `job_kill`

`job_kill` stops the selected background job and its full process tree. If the job has already exited, the call returns the existing exit status.

### `job_list`

`job_list` defaults to `status="running"`. Use `status="finished"` to inspect retained exited or interrupted records, and `status="all"` only when both lifecycles are needed. Results are newest first within each lifecycle. `offset` continues a page; `limit` overrides the saved page size for one call.

Finished records have no time-to-live. FastCtx retains them until the current user's `fastshell.job_storage_limit_mib` limit requires eviction of the oldest finished records. The default is 1024 MiB. Running jobs and their records are never evicted; `fastshell.max_running_jobs` limits concurrent jobs across all FastCtx sessions for that user and defaults to 128. `fastshell.job_list_limit` is the default page size (20, valid range 1–100). All three settings take effect immediately when saved and do not require reconnecting; the TUI presets for page size are 10 / 20 / 50 / 100.

The TUI **Jobs** dashboard scans this same current-user registry but shows only jobs that are currently running, aggregated from every FastCtx session and TUI instance. A finished job disappears with a short notice that its retained output remains available to the agent through `job_output`. Jobs are grouped by a source tag with workspace and runtime-process context. Fixed list columns keep relative age and job ids aligned, while long ASCII or CJK commands end with an ellipsis at one shared edge. The detail panel shows the exact UTC start time to the second and a live `HH:MM:SS` elapsed time. Horizontal and vertical output navigation remains available; one width-aware footer row keeps the essential keys visible and adds `←/→ output`, `PgUp/PgDn scroll`, and `F follow` when space permits. ChatGPT / Codex does not expose conversation titles or ids to the MCP server, so FastCtx does not invent one.

## Security and privacy

The FastCtx MCP server inherits the local permissions of the host process.

| Capability | Default state | Access scope |
|---|---|---|
| `inspect_local_file` / `grep` / `glob` | Enabled | Local files readable by the host process |
| `replace` | Enabled | Local file writes with dry-run, CAS, and atomic replacement safeguards |
| Bash tools | Disabled | Bash command execution after the user enables them |
| TUI update check | Enabled for npm and GitHub Release launches | Version metadata from `registry.npmjs.org` and GitHub's `releases/latest` web redirect; downloads require explicit confirmation |
| MCP runtime network requests | None | `serve`, private local proxy traffic, and tool calls perform no telemetry or update traffic |

The startup check sends the FastCtx version, normal HTTPS request metadata, and npm's standard registry request; it never sends repository paths, job data, or file contents. Background jobs persist their command, working directory, retained output prefix, truncation state, and exit status only in the current user's private `~/.fastctx/jobs/` directory. Proxy-to-control-center traffic stays on an owner-private Unix-domain socket or Windows named pipe. FastCtx does not upload this data. Bash commands can access the network according to the command itself. Prebuilt binaries include the PDF engine.

The table above describes a machine that has not been enrolled in a World, which is every machine until you enroll one yourself. Enrolling is a deliberate act — `fastctx node enroll` with an invite you created — and it adds exactly one outbound connection, made by the `fastctx node` daemon to a hub you run and own. `fastctx serve` still makes no network requests of its own. The hub routes on plaintext headers and cannot read what members send each other: message bodies are encrypted under a World key the hub never holds. There is no FastCtx server and no vendor server anywhere on this path, and nothing is uploaded that you did not ask for — a remote tool call carries the arguments you passed and returns the output that call produced.

The MCP server runs outside the host filesystem sandbox. Use an approval mode when every write and command execution should be reviewed:

```toml
[mcp_servers.fastctx]
default_tools_approval_mode = "writes"
```

- `writes`: review `replace` and shell execution tools;
- `prompt`: review every tool call.

`replace` is published with the default file tools. The host's read-only mode covers the host's own tools. MCP writes still run with the server process permissions. Set `writes` or `prompt` when the workflow depends on a read-only boundary.

## What FastCtx changes

FastCtx uses or manages these paths and settings:

- `~/.fastctx/bin/fastctx(.exe)`: the stable self-installed binary;
- `~/.fastctx/config.toml`: control terminal settings and the connection receipt;
- `~/.fastctx/jobs/`: persistent background-job records and current-format full output logs, created on demand by `run_background`;
- one `fastctx` MCP registration and one marker-owned guidance block for each connected target, in that target's documented current-user configuration and guidance files;
- for Codex, `[mcp_servers.fastctx]`, the `mcp__fastctx` direct-only namespace entry, the marker-delimited block in `~/.codex/AGENTS.md`, `tool_timeout_sec = 300`, and the confirmed `tool_output_token_limit`;
- a schema-v2 receipt that records ownership, contract hashes, and the exact enabled-tool set independently for every target.

FastCtx preserves unrelated TOML, JSON/JSONC, YAML, Markdown, and rule-file content. Target Disconnect removes only content still owned by FastCtx and preserves the shared binary, settings, jobs, and other agents. Full Unapply stops running jobs before removing `~/.fastctx/`. User changes made after Apply are surfaced as ownership drift rather than overwritten or silently removed.

## License

FastCtx is licensed under the Apache License 2.0.

If you redistribute FastCtx, bundle it into another product, or build on top of it, Section 4(d) requires you to reproduce the attribution notice in [`NOTICE`](./NOTICE) wherever third-party notices normally appear — for a source repository, that means your README. That notice credits https://github.com/yc-duan/fastctx and states that your changes are your own work and your sole responsibility, carrying no endorsement or liability from this project's author. Section 4(b) separately requires files you modified to carry prominent notices that you changed them.

Third-party notices for the bundled Pdfium build are listed in [`THIRD_PARTY_LICENSES.md`](./THIRD_PARTY_LICENSES.md).

## Contact

FastCtx is created and maintained by [yc-duan](https://github.com/yc-duan). For integration, redistribution, partnership, or anything else, feel free to reach out: dy2958830371@gmail.com.

## Acknowledgements

Thanks to the [linuxdo](https://linux.do/) community for discussion, sharing, and feedback.
