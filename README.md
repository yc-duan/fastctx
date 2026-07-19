# FastCtx

**English** | [简体中文](./README.zh-CN.md)

### Fast, context-efficient repository tools for AI agents.

FastCtx is a local Rust tool runtime. It provides file reading, content search, file discovery, batch replacement, and Bash command execution through MCP.

Repository operations run in a persistent process with stable input schemas and output formats. The model can gather the context it needs in fewer steps and spend more attention on understanding code, planning changes, and verifying results.

```console
npm install --global fastctx
fastctx
```

The `fastctx` command opens the control terminal. Review the proposed changes, select **Apply**, then start a new ChatGPT / Codex session.

FastCtx currently provides first-class setup for ChatGPT App and Codex CLI. Any MCP client can also register `fastctx serve` directly.

## What FastCtx solves

Coding agents often assemble shell commands on the fly when they access a repository. They have to handle quotes, escaping, paths, and platform differences, then extract the useful information from terminal output. A simple file read or symbol search can take several tool calls just to confirm that the command is correct and the result is complete.

This work consumes context and reasoning. The model tracks the code problem and the tool mechanics at the same time: whether the PowerShell syntax is correct, whether a path was escaped correctly, whether the encoding produced mojibake, and whether the host truncated a long result. More tool overhead leaves less room for the repository itself.

FastCtx turns common repository operations into structured input and output. The model provides parameters such as a path, pattern, range, and mode. The Rust runtime handles command construction, directory traversal, encoding, pagination, and output boundaries.

The tools cover the main parts of a coding task:

- `read` reads text, images, PDFs, and raw bytes;
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

1. Adjust the output tier;
2. Enable **Bash terminal** when needed;
3. Set current-user background-job storage, concurrency, and AI list page limits;
4. Inspect every currently running job across FastCtx sessions, follow its output, and stop it on the **Jobs** screen;
5. Review every configuration change on the Apply screen;
6. Apply the changes and restart the ChatGPT / Codex session.

Apply copies the current binary to `~/.fastctx/bin/` and points the host configuration at that stable path. The applied setup keeps working after npm cache cleanup or upgrades.

The full-screen terminal opens immediately while FastCtx checks its launch channel in a background thread. Successful results are cached for 24 hours in machine-private storage outside `~/.fastctx`. npm launches query the exact launcher package through a fresh isolated cache with `--prefer-online`; direct GitHub Release executables read the stable tag from GitHub's `releases/latest` web redirect. Available updates remain visible from the main menu and open a dedicated screen with **Update & restart** and **Continue**.

If GitHub has published a release but npm has not exposed the matching version yet, FastCtx shows a propagation screen instead of trusting stale metadata. **Retry** always uses another isolated cache; it never clears or mutates the user's normal npm cache. Transient network or rate-limit failures stay quiet and are recorded under **Status**; malformed publication metadata produces one warning. Status also offers a manual check that bypasses the 24-hour TTL. An accepted npm update installs the exact version with lifecycle scripts disabled. A GitHub Release update downloads this repository's platform archive and aggregate `SHA256SUMS`, verifies the archive before safely extracting the binary, probes the downloaded version, replaces the executable atomically, and rolls back when restart health fails. A failed npm update restores the exact previous package version; every failed update reopens the previous TUI with a warning. After a successful restart, an owned `~/.fastctx/bin/` Apply copy is synchronized; externally changed copies are left untouched.

`cargo install` builds and the internal `~/.fastctx/bin/` runtime are not self-updated. Set `FASTCTX_DISABLE_UPDATE_CHECK=1` to disable the TUI startup check.

**Unapply** stops FastCtx process images running from the managed bin directory, removes the configuration managed by FastCtx, and deletes its managed data. Shared settings changed by the user after Apply are preserved.

### One-off run

```console
npx fastctx
```

`npx` opens the same control terminal without a global installation. Apply still copies the binary to `~/.fastctx/bin/`, so the applied setup keeps working after the npx cache is cleaned; only the `fastctx` command itself requires the global installation.

### Non-interactive use

```console
fastctx apply --tier standard --yes
fastctx status
fastctx jobs
fastctx jobs kill j-a1b2c3
fastctx unapply --yes
```

- `apply`: install FastCtx and write the configuration;
- `status`: check the configuration, binary, and MCP handshake;
- `jobs`: list running background jobs;
- `jobs kill <job_id>`: stop one background job and its full process tree;
- `unapply`: remove the content managed by FastCtx;
- `lang <code>`: set the control terminal language.

`status` uses three states: `[PASS]`, `[INFO]`, and `[FAIL]`. A `[FAIL]` result returns a non-zero exit code.

### Other distribution channels

```console
cargo install fastctx --locked
```

GitHub Releases provides a zip archive for Windows x64 and executable-preserving tar.gz archives for Linux x64, macOS x64, and macOS arm64. Every archive includes the binary and license notices; verify it with the release's aggregate `SHA256SUMS`.

## Tools

FastCtx provides nine MCP tools:

| Tool | Purpose |
|---|---|
| `read` | Read text, images, PDFs, and raw bytes from any file |
| `grep` | Search contents in a file or repository tree |
| `glob` | Find files by path pattern |
| `replace` | Apply mechanical batch replacements to files or a repository tree |
| `run` | Run a Bash command in the foreground |
| `run_background` | Start a background Bash job |
| `job_output` | Read new output from a background job |
| `job_kill` | Stop the full process tree of a background job |
| `job_list` | Rediscover running and retained finished jobs |

`read`, `grep`, `glob`, and `replace` are published by default. The other five tools are enabled with the **Bash terminal** setting in the control terminal. Once enabled, they share the same `mcp__fastctx__*` namespace as the file tools.

### `read`

`read` returns 1-based line numbers for text and supports paging:

```json
{
  "file_path": "V:/repo/src/main.rs",
  "offset": 120,
  "limit": 40
}
```

```text
120	fn main() {
121	    ...
159	}

(Partial: lines 120-159 of 512 shown. Continue with offset=160.)
```

The continuation parameters in the final status line can be used directly in the next call. In this example, pass `offset=160` to read the next section.

`read` also supports:

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
V:/repo/src/edit/locks.rs
62-/// Cross-process lock keyed by file identity.
63:pub fn acquire_path_lock(identity: &PathIdentity) -> LockGuard {
64-    ...

(Complete: all 1 result shown.)
```

`output_mode` has four values:

- `files_with_matches`: return matching files;
- `content`: show matches grouped by file;
- `count`: return the occurrence count for each file;
- `summary`: scan the full target and return global totals.

Searches respect `.gitignore` and `.ignore` by default, include hidden files, and exclude `.git` and binary files. Common filters include `glob`, `type`, `case_insensitive`, `multiline`, and `context`. Page through results with `head_limit` and `offset`.

Files with uncertain encodings appear in a skip report with their path, reason, and resolution parameters. Use `encoding` for a single file and `fallback_encoding` for a directory search.

### `glob`

`glob` finds files with a pattern relative to the search root:

```json
{
  "pattern": "**/*.toml",
  "path": "V:/repo",
  "sort": "modified"
}
```

```text
V:/repo/crates/core/Cargo.toml
V:/repo/Cargo.toml

(Complete: all 2 files shown.)
```

Main parameters:

- `filter_mode: "project"`: apply ignore rules, exclude `.git`, and keep hidden files visible;
- `filter_mode: "all"`: list every file;
- `sort: "path"`: use a stable path order;
- `sort: "modified"`: order files from newest to oldest;
- `offset` / `limit`: page through the result set.

### `replace`

`replace` handles mechanical, deterministic batch changes such as symbol renames, import rewrites, configuration key migrations, and fixed-pattern deletion. Generated code changes and per-location semantic edits are handled by the host's `apply_patch` tool.

```json
{
  "pattern": "old_name\\(",
  "replacement": "new_name(",
  "path": "V:/repo/src",
  "glob": "**/*.rs",
  "dry_run": true
}
```

```text
...

(Complete: dry run — 12 matches in 3 files; nothing written.)
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

Output uses bounded memory. When output exceeds the response capacity, the final status line reports the truncated range and gives a path to the complete result: redirect the command output to a file, then page through it with `read`.

### `run_background`

`run_background` starts a background Bash job and returns a job id immediately. It is useful for builds, tests, development servers, and other long-running commands.

Each job is owned by a detached supervisor rather than by the MCP server. It keeps running across server exits, ChatGPT / Codex restarts, and session changes until the command exits or `job_kill` stops it. There is no background timeout parameter.

Output and exit status are stored under `~/.fastctx/jobs/`, so another FastCtx server can resume the same job by id. Each job keeps an 8 MiB rolling output window; redirect the command to a file when a complete log is required.

### `job_output`

`job_output` reads new output from a background job, including jobs started in earlier sessions, and reports `running`, `exited`, or `interrupted`. `wait_ms` enables long polling. `after_seq` re-anchors the read position and keeps paging stable when a call is retried.

Keep calling it until the final line says `Complete`. When the ring buffer evicts output, the response reports the number of lost lines and recommends redirecting command output to a file for a complete log.

### `job_kill`

`job_kill` stops the selected background job and its full process tree. If the job has already exited, the call returns the existing exit status.

### `job_list`

`job_list` defaults to `status="running"`. Use `status="finished"` to inspect retained exited or interrupted records, and `status="all"` only when both lifecycles are needed. Results are newest first within each lifecycle. `offset` continues a page; `limit` overrides the saved page size for one call.

Finished records have no time-to-live. FastCtx retains them until the current user's `fastshell.job_storage_limit_mib` limit requires eviction of the oldest finished records. The default is 1024 MiB. Running jobs and their records are never evicted; `fastshell.max_running_jobs` limits concurrent jobs across all FastCtx sessions for that user and defaults to 128. `fastshell.job_list_limit` is the default page size (20, valid range 1–100). All three settings take effect immediately when saved and do not require Apply; the TUI presets for page size are 10 / 20 / 50 / 100.

The TUI **Jobs** dashboard scans this same current-user registry but shows only jobs that are currently running, aggregated from every FastCtx server and TUI instance. A finished job disappears with a short notice that its retained output remains available to the agent through `job_output`. Jobs are grouped by an honest source-session tag with workspace, server PID, and parent-process context. Fixed list columns keep relative age and job ids aligned, while long ASCII or CJK commands end with an ellipsis at one shared edge. The detail panel shows the exact UTC start time to the second and a live `HH:MM:SS` elapsed time. Horizontal and vertical output navigation remains available; one width-aware footer row keeps the essential keys visible and adds `←/→ output`, `PgUp/PgDn scroll`, and `F follow` when space permits. ChatGPT / Codex does not expose conversation titles or ids to the MCP server, so FastCtx does not invent one.

## Security and privacy

The FastCtx MCP server inherits the local permissions of the host process.

| Capability | Default state | Access scope |
|---|---|---|
| `read` / `grep` / `glob` | Enabled | Local files readable by the host process |
| `replace` | Enabled | Local file writes with dry-run, CAS, and atomic replacement safeguards |
| Bash tools | Disabled | Bash command execution after the user enables them |
| TUI update check | Enabled for npm and GitHub Release launches | Version metadata from `registry.npmjs.org` and GitHub's `releases/latest` web redirect; downloads require explicit confirmation |
| MCP runtime network requests | None | `serve` and tool calls perform no telemetry or update traffic |

The startup check sends the FastCtx version, normal HTTPS request metadata, and npm's standard registry request; it never sends repository paths, job data, or file contents. Background jobs persist their command, working directory, rolling stdout/stderr, and exit status only in the current user's private `~/.fastctx/jobs/` directory. FastCtx does not upload this data. Bash commands can access the network according to the command itself. Prebuilt binaries include the PDF engine.

The MCP server runs outside the host filesystem sandbox. Use an approval mode when every write and command execution should be reviewed:

```toml
[mcp_servers.fastctx]
default_tools_approval_mode = "writes"
```

- `writes`: review `replace` and shell execution tools;
- `prompt`: review every tool call.

`replace` is published with the default file tools. The host's read-only mode covers the host's own tools. MCP writes still run with the server process permissions. Set `writes` or `prompt` when the workflow depends on a read-only boundary.

## Codex configuration

Codex code mode places regular MCP tools inside an execution container. Aggregated results from multiple calls can be truncated in the middle by the host. This setting keeps FastCtx as a direct top-level namespace:

```toml
[features.code_mode]
direct_only_tool_namespaces = ["mcp__fastctx"]
```

Apply maintains this setting automatically and writes a guidance block with explicit markers to `~/.codex/AGENTS.md` so the model prefers the FastCtx tools.

FastCtx uses an internal output budget of 8,500 tokens by default, around 85% of Codex's default tool output limit. The control terminal provides three tiers:

- `Standard`: the default tier;
- `High`: raises the global Codex tool output limit;
- `Extra High`: provides the largest per-call output space.

Higher output tiers allow larger results per call and consume context faster. Choose a tier according to the task.

<details>
<summary>Manual MCP registration</summary>

```toml
[mcp_servers.fastctx]
command = "C:/absolute/path/to/fastctx.exe"
args = ["serve"]
startup_timeout_sec = 120

[features.code_mode]
direct_only_tool_namespaces = ["mcp__fastctx"]
```

Enable the Bash tools with:

```toml
args = ["serve", "--enable-shell"]
```

When the binary is on PATH, `command` can be set to `fastctx`. The compatibility npm package `codex-fastctx` installs the same `fastctx` command.

</details>

## What FastCtx changes

FastCtx uses or manages these paths and settings:

- `~/.fastctx/bin/fastctx(.exe)`: the stable self-installed binary;
- `~/.fastctx/config.toml`: control terminal settings and the Apply receipt;
- `~/.fastctx/jobs/`: persistent background-job records and rolling output, created on demand by `run_background`;
- `[mcp_servers.fastctx]` in `~/.codex/config.toml`;
- the `mcp__fastctx` entry in `direct_only_tool_namespaces`;
- the marker-delimited FastCtx block in `~/.codex/AGENTS.md`;
- the selected `tool_output_token_limit` value after user confirmation.

FastCtx edits existing TOML with `toml_edit`, preserving comments, formatting, and unrelated configuration. Unapply removes entries according to write ownership and preserves later user changes. It stops running background jobs before removing `~/.fastctx/`.

## License

FastCtx is dual-licensed under MIT OR Apache-2.0. Redistributions must retain the [`NOTICE`](./NOTICE) file. Third-party notices for the bundled Pdfium build are listed in [`THIRD_PARTY_LICENSES.md`](./THIRD_PARTY_LICENSES.md).

## Contact

FastCtx is created and maintained by [yc-duan](https://github.com/yc-duan). For integration, redistribution, partnership, or anything else, feel free to reach out: dy2958830371@gmail.com.

## Acknowledgements

Thanks to the [linuxdo](https://linux.do/) community for discussion, sharing, and feedback.
