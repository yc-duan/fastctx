# FastCtx

[English](./README.md) | **简体中文**

### 面向 AI 智能体的快速、上下文高效的仓库工具。

FastCtx 是一个纯本地的 Rust 工具运行时，通过 MCP 提供文件读取、内容搜索、文件查找、批量替换和 Bash 命令执行能力。

仓库操作由常驻进程完成，输入参数和返回结果都有稳定结构。模型可以用更少的步骤取得所需上下文，并把精力留给代码理解、修改决策和结果验证。

```console
npx fastctx
```

命令会打开控制终端。检查变更后选择 **Apply**，再启动新的 ChatGPT / Codex 会话即可使用。

当前优先支持 ChatGPT App 与 Codex CLI。任何 MCP client 也可以直接注册 `fastctx serve`。

## FastCtx 解决什么

编码智能体访问仓库时，经常要临时拼接 shell 命令，处理引号、转义、路径和平台差异，再从终端文本中提取真正需要的信息。一次简单的文件读取或符号搜索，也可能让模型花掉多轮工具调用来确认命令是否正确、结果是否完整。

这些步骤会占用上下文和推理过程。模型需要同时关注代码问题与工具问题：PowerShell 语法有没有写对，路径是否被错误转义，编码是否产生乱码，长输出是否被宿主截断。工具链越绕，留给仓库本身的信息空间越少。

FastCtx 将常见仓库操作整理成结构化输入输出。模型提供路径、pattern、范围和模式等参数，工具返回格式稳定、边界清楚的结果。命令拼接、目录遍历、编码处理、分页和输出收口由 Rust 运行时负责。

这组工具覆盖编码任务的主要环节：

- `read` 读取文本、图片、PDF 和原始字节；
- `grep` 搜索文件内容；
- `glob` 查找文件；
- `replace` 执行机械批量替换；
- `run`、`run_background`、`job_output`、`job_kill`、`job_list` 执行 Bash 命令并管理可跨会话的持久后台任务。

这能极大减轻模型对工具本身的注意力负担（比如关注在处理 PowerShell 命令正确性上），提高上下文有效性，提高任务完成速度和质量。

## 安装

### 使用 npx

需要 Node.js 18 或更高版本：

```console
npx fastctx
```

首次启动会进入全屏控制终端。界面支持 17 种语言，主要操作包括：

1. 调整输出档位；
2. 按需启用 **Bash terminal**；
3. 设置当前用户的后台任务存储、并发上限与 AI 列表分页条数；
4. 在 **Jobs** 页面汇总查看所有 FastCtx 会话中正在运行的任务、跟随输出，并按需终止；
5. 在 Apply 页面检查全部配置变更；
6. 确认应用并重启 ChatGPT / Codex 会话。

Apply 会把当前二进制复制到 `~/.fastctx/bin/`，并让宿主配置指向这个稳定路径。清理或升级 npm 缓存后，已经应用的配置仍然有效。

全屏终端会立即打开，同时 FastCtx 在后台线程中按本次启动来源检查更新。成功结果会在 `~/.fastctx` 之外的机器级私有存储中缓存 24 小时。npm 启动会针对实际 launcher 包，使用全新的独立缓存和 `--prefer-online` 查询；直接下载的 GitHub Release 程序会从 GitHub 的 `releases/latest` 网页重定向读取稳定 tag。发现新版本后，主菜单会持续显示入口，可进入独立界面选择 **更新并重启** 或 **继续使用**。

如果 GitHub 已发布新版本、但 npm 暂时还没有显示对应版本，FastCtx 会明确进入“等待 npm 同步”界面，而不是相信陈旧结果。每次 **重试** 都会再建一个独立缓存，不清理、也不修改用户原有 npm 缓存。网络、限流等瞬态失败保持安静，并记录在 **状态** 页面；发布元数据结构异常只警告一次。状态页面也提供绕过 24 小时缓存的手动检查。确认 npm 更新后只安装精确版本，并禁用生命周期脚本。GitHub Release 更新会下载本仓库对应平台的归档与汇总 `SHA256SUMS`，先校验归档，再安全解出二进制、执行版本探测并原子替换；重启健康检查失败会回滚。npm 更新失败会精确恢复先前包版本；任何更新事务失败都会重新打开旧版 TUI 并显示警告。更新成功后，由 FastCtx 拥有的 `~/.fastctx/bin/` Apply 副本会同步更新，外部改写过的副本保持不动。

`cargo install` 构建和内部 `~/.fastctx/bin/` runtime 不会自行更新。可设置 `FASTCTX_DISABLE_UPDATE_CHECK=1` 关闭 TUI 启动检查。

**Unapply** 会终止从受管 bin 目录运行的 FastCtx 进程镜像，撤销 FastCtx 管理的配置并删除受管数据。用户在 Apply 之后修改的共享设置会保留。

### 全局安装

```console
npm install --global fastctx
fastctx
```

### 非交互使用

```console
fastctx apply --tier standard --yes
fastctx status
fastctx jobs
fastctx jobs kill j-a1b2c3
fastctx unapply --yes
```

- `apply`：安装并写入配置；
- `status`：检查配置、二进制和 MCP 握手；
- `jobs`：列出运行中的后台任务；
- `jobs kill <job_id>`：终止指定后台任务及其完整进程树；
- `unapply`：撤销 FastCtx 管理的内容；
- `lang <code>`：设置控制终端语言。

`status` 使用 `[PASS]`、`[INFO]`、`[FAIL]` 三种状态。出现 `[FAIL]` 时退出码为非零。

### 其他分发方式

```console
cargo install fastctx --locked
```

GitHub Releases 为 Windows x64 提供 zip，为 Linux x64、macOS x64 和 macOS arm64 提供保留执行位的 tar.gz。每个归档都包含二进制与许可声明，并由 Release 的汇总 `SHA256SUMS` 校验。

## 工具

FastCtx 提供九个同级 MCP 工具：

| 工具 | 用途 |
|---|---|
| `read` | 读取文本、图片、PDF 和任意文件的原始字节 |
| `grep` | 搜索单个文件或项目树中的内容 |
| `glob` | 按路径模式查找文件 |
| `replace` | 对文件或项目树执行机械批量替换 |
| `run` | 前台执行 Bash 命令 |
| `run_background` | 启动后台 Bash 任务 |
| `job_output` | 增量读取后台任务输出 |
| `job_kill` | 终止后台任务的整个进程树 |
| `job_list` | 找回运行中及已留存的终态任务 |

`read`、`grep`、`glob`、`replace` 默认发布。其余五个工具通过控制终端中的 **Bash terminal** 开关启用；启用后，它们与文件工具位于同一个 `mcp__fastctx__*` 命名空间。

### `read`

`read` 读取文本时返回 1 基行号，并支持分页：

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

终态中的续读参数可以直接用于下一次调用。上例继续传入 `offset=160` 即可读取后续内容。

`read` 还支持：

- PNG、JPG、GIF、WebP、BMP 图片；
- PDF 文本层和页面渲染图；
- 任意文件的分页 hex 视图；
- UTF-8、BOM 以及常见本地编码。

自动编码判定采用证据充分的结果。编码存在歧义时，错误信息会列出候选和重试方式，可以使用 `encoding` 明确指定：

```json
{
  "file_path": "V:/repo/docs/legacy.txt",
  "encoding": "gbk"
}
```

读取二进制文件时，可以使用：

```json
{
  "file_path": "V:/repo/data/cache.bin",
  "view": "hex"
}
```

### `grep`

`grep` 使用 ripgrep 同源的 Rust regex 引擎：

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

`output_mode` 有四种取值：

- `files_with_matches`：返回命中文件；
- `content`：按文件分组展示匹配内容；
- `count`：返回每个文件的 occurrence 数；
- `summary`：完成全量扫描后返回总数。

搜索默认尊重 `.gitignore` 和 `.ignore`，包含隐藏文件，排除 `.git` 与二进制文件。常用筛选参数包括 `glob`、`type`、`case_insensitive`、`multiline` 和 `context`。结果通过 `head_limit` 与 `offset` 分页。

编码判定存疑的文件会进入跳过报告，报告中包含路径、原因和解决参数。单文件可以传 `encoding`，目录搜索可以传 `fallback_encoding`。

### `glob`

`glob` 按相对搜索根的 pattern 查找文件：

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

主要参数：

- `filter_mode: "project"`：应用忽略规则，排除 `.git`，保留隐藏文件；
- `filter_mode: "all"`：列出全部文件；
- `sort: "path"`：按稳定路径顺序排列；
- `sort: "modified"`：按修改时间从新到旧排列；
- `offset` / `limit`：分页读取结果。

### `replace`

`replace` 适合处理机械、确定性的批量修改，例如符号重命名、import 改写、配置键迁移和固定模式删除。生成式代码修改与逐处语义编辑由宿主的 `apply_patch` 处理。

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

`replace` 会在写入前冻结候选集并统计完整命中数。`dry_run` 用于预览，`max_replacements` 用于限制影响范围。

每个文件在提交前都会重新校验。写入采用同目录原子替换，并保留原编码、BOM、换行风格、尾随换行、Unix mode 和未修改字节。并发改动会让对应文件进入失败报告，其他文件继续处理。

### `run`

`run` 前台执行 Bash 命令，返回合并后的 stdout、stderr 和退出码。Windows 使用 Git Bash，macOS 和 Linux 使用系统 Bash。

```json
{
  "command": "cargo test --quiet 2>&1 | tail -n 40",
  "timeout_ms": 180000
}
```

命令运行在非交互环境中。安装、确认和编辑类命令需要带 `-y`、`--no-edit` 等参数。非零退出码会作为执行结果返回。

在 Windows 上，FastCtx 自己创建的所有非交互子进程都默认以无控制台窗口方式启动，包括 Bash 探测、前后台 Bash、分离监督进程和 doctor 探针；用户无需记住任何隐藏窗口参数。若命令本身显式启动 GUI 或新的终端窗口，该可见效果仍会按命令意图发生。

输出使用有界内存缓冲。响应容量不足时，终态会说明截断范围，并给出完整输出的处理方式：将命令输出重定向到文件，再用 `read` 分页查看。

### `run_background`

`run_background` 启动后台 Bash 任务并立即返回 job id，适合构建、测试、开发服务器和其他长时间运行的命令。

每个任务由独立监督进程管理，不归 MCP server 所有。server 退出、ChatGPT / Codex 重启或切换会话时，任务仍会继续运行，直到命令自然结束或被 `job_kill` 终止。后台任务不提供超时参数。

输出与退出状态保存在 `~/.fastctx/jobs/`，因此新的 FastCtx server 可以凭同一 job id 继续读取。每个任务保留 8 MiB 滚动输出窗口；需要完整日志时，应把命令输出重定向到文件。

### `job_output`

`job_output` 增量读取后台任务的新输出，也能读取此前会话启动的任务，并返回 `running`、`exited` 或 `interrupted` 状态。`wait_ms` 支持长轮询；`after_seq` 可以重新锚定读取位置，在调用重试时保持分页稳定。

持续调用，直到结果末行显示 `Complete`。缓冲区发生淘汰时，响应会报告已经丢失的行数，并提示将命令输出重定向到文件以保留完整日志。

### `job_kill`

`job_kill` 终止指定后台任务及其整个进程树。任务已经自然退出时，调用会返回现有退出状态。

### `job_list`

`job_list` 默认使用 `status="running"`，只返回运行中的任务。显式传 `status="finished"` 可查看保留的已退出或已中断记录；确实需要两种生命周期时才用 `status="all"`。每种生命周期内按时间从新到旧排序；`offset` 用于续页，`limit` 只覆盖本次调用的已保存页大小。

终态记录没有 TTL。只有当前用户的 `fastshell.job_storage_limit_mib` 超限时，FastCtx 才会从最旧终态记录开始回收；默认上限为 1024 MiB。运行中的任务及其记录永不被回收。`fastshell.max_running_jobs` 限制该用户全部 FastCtx 会话合计的并发后台任务数，默认 128。`fastshell.job_list_limit` 是默认每页数量，默认 20、有效范围 1–100，TUI 预设为 10 / 20 / 50 / 100。三项设置保存后都立即生效，无需 Apply。

TUI 的 **Jobs** dashboard 直接扫描同一份当前用户 registry，但只显示当前仍在运行的任务，并汇总所有 FastCtx server 与 TUI 实例。任务结束后会从列表消失，同时短暂提示其保留输出仍可由智能体通过 `job_output` 读取。界面按真实来源会话标签分组，并显示 workspace、server PID 与父进程信息；列表用固定列对齐相对时间和 job id，过长的 ASCII/CJK 命令会在同一右边界显示省略号。右侧详情显示精确到秒的 UTC 开始时间和实时 `HH:MM:SS` 已运行时长。输出仍支持水平和纵向移动；页脚固定为一行，优先保留关键按键，并在宽度允许时补充 `←/→ 输出`、`PgUp/PgDn 滚动` 与 `F 跟随`。ChatGPT / Codex 没有向 MCP server 暴露对话标题或 id，因此 FastCtx 不会伪造一个对话名。

## 安全与隐私

FastCtx MCP server 继承宿主进程的本地权限。

| 能力 | 默认状态 | 访问范围 |
|---|---|---|
| `read` / `grep` / `glob` | 开启 | 宿主进程有权读取的本地文件 |
| `replace` | 开启 | 本地文件写入，带 dry-run、CAS 和原子替换保护 |
| Bash 工具 | 关闭 | 用户启用后可执行 Bash 命令 |
| TUI 更新检查 | npm 与 GitHub Release 启动时开启 | 从 `registry.npmjs.org` 与 GitHub 的 `releases/latest` 网页重定向获取版本元数据；下载必须由用户确认 |
| MCP runtime 网络请求 | 无 | `serve` 与工具调用不产生遥测或更新流量 |

启动检查只会发送 FastCtx 版本、常规 HTTPS 请求元数据，以及 npm 的标准仓库请求；不会发送仓库路径、任务数据或文件内容。后台任务的命令、工作目录、滚动 stdout/stderr 与退出状态只保存在当前用户的私有目录 `~/.fastctx/jobs/` 中，FastCtx 不会上传这些数据。Bash 命令仍可按照命令本身访问网络。预构建版本已经内嵌 PDF 引擎。

MCP server 位于宿主文件沙箱之外。需要逐次确认写入和命令执行时，可以配置：

```toml
[mcp_servers.fastctx]
default_tools_approval_mode = "writes"
```

- `writes`：确认 `replace` 和 shell 执行工具；
- `prompt`：确认全部工具调用。

`replace` 默认随文件工具发布。宿主的 read-only 模式只覆盖宿主自身工具，MCP 写入仍按 server 权限执行。依赖只读边界时，请同时设置 `writes` 或 `prompt`。

## Codex 配置说明

Codex code mode 会把普通 MCP 工具放入执行容器，多次调用的聚合结果可能被宿主从中间截断。下面的配置让 FastCtx 保持顶层直达：

```toml
[features.code_mode]
direct_only_tool_namespaces = ["mcp__fastctx"]
```

Apply 会自动维护这一项，并在 `~/.codex/AGENTS.md` 中写入带边界标记的引导段，让模型优先使用 FastCtx 工具。

FastCtx 默认使用 8500 token 的内部输出预算，约为 Codex 默认工具输出上限的 85%。控制终端提供三个档位：

- `Standard`：默认档；
- `High`：提高 Codex 全局工具输出上限；
- `Extra High`：提供最大的单次工具输出空间。

输出档位越高，单次结果越大，上下文消耗也越快。请按任务实际需要调整。

<details>
<summary>手动注册 MCP</summary>

```toml
[mcp_servers.fastctx]
command = "C:/absolute/path/to/fastctx.exe"
args = ["serve"]
startup_timeout_sec = 120

[features.code_mode]
direct_only_tool_namespaces = ["mcp__fastctx"]
```

启用 Bash 工具时：

```toml
args = ["serve", "--enable-shell"]
```

二进制位于 PATH 时，`command` 可以直接填写 `fastctx`。兼容 npm 包 `codex-fastctx` 安装同一个 `fastctx` 命令。

</details>

## FastCtx 会修改什么

FastCtx 使用或管理以下内容：

- `~/.fastctx/bin/fastctx(.exe)`：稳定的自安装二进制；
- `~/.fastctx/config.toml`：控制终端配置与 Apply 回执；
- `~/.fastctx/jobs/`：由 `run_background` 按需创建的持久后台任务记录与滚动输出；
- `~/.codex/config.toml` 中的 `[mcp_servers.fastctx]`；
- `direct_only_tool_namespaces` 中的 `mcp__fastctx` 元素；
- `~/.codex/AGENTS.md` 中带边界标记的 FastCtx 段；
- 用户确认后的 `tool_output_token_limit` 档位值。

FastCtx 使用 `toml_edit` 修改已有 TOML，保留注释、格式和其他配置。Unapply 按写入所有权逐项撤销，用户后续改动会保留；删除 `~/.fastctx/` 前会先终止所有运行中的后台任务。

## License

FastCtx 采用 MIT OR Apache-2.0 双许可证。再分发时须保留 [`NOTICE`](./NOTICE) 文件。内嵌 Pdfium 的第三方许可见 [`THIRD_PARTY_LICENSES.md`](./THIRD_PARTY_LICENSES.md)。

## 联系方式

FastCtx 由 [yc-duan](https://github.com/yc-duan) 创建和维护。集成、再分发、合作或任何其他事宜，欢迎联系：dy2958830371@gmail.com。

## 致谢

感谢 [linuxdo](https://linux.do/) 社区的讨论、分享与反馈。
