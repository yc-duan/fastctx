# FastCtx Windows ARM64 社区版

面向 Codex、ChatGPT 和其他 MCP 客户端的本地仓库工具运行时。

> [!IMPORTANT]
> 这是一个非官方 Windows ARM64 社区构建。FastCtx 的项目设计、主体代码和
> 长期维护均来自原作者 [yc-duan](https://github.com/yc-duan)。请关注并支持
> [原项目 yc-duan/fastctx](https://github.com/yc-duan/fastctx)。

## 项目信息

- 已发布社区版本（基于上游 v0.2.1）：[Windows ARM64 v1](https://github.com/Simplepine/fastctx/releases/tag/v1)
- 社区仓库：[Simplepine/fastctx](https://github.com/Simplepine/fastctx)
- 原项目：[yc-duan/fastctx](https://github.com/yc-duan/fastctx)
- 原作者：[yc-duan](https://github.com/yc-duan)
- 当前分支上游基线：`v0.2.2`
- ARM64 基础适配提交：[`1db4537`](https://github.com/Simplepine/fastctx/commit/1db453750097add2971cc4d1a919fd5bae6ecfad)
- 架构：`aarch64-pc-windows-msvc`
- 许可证：MIT OR Apache-2.0

当前分支构建的程序会如实显示：

```text
fastctx 0.2.2
```

GitHub 标签 `v1` 仍是基于上游 v0.2.1 的历史发布；下方快速开始中的
v1 下载链接也仍指向该版本，直到 v0.2.2 社区资产正式发布。

## 当前 v0.2.2 同步状态

- 已合入上游 v0.2.2 的批量读取、持久完整后台日志、状态行和更新检查改进
- 保留 Scoop Git Bash 发现、中文路径覆盖和 Windows ARM64 无 npm 平台包兼容
- Windows ARM64 后台监督进程使用 30 秒启动窗口，其他平台保持上游 10 秒
- `--no-default-features` 原生 ARM64 release 构建通过，PDF 仍关闭
- 651 项测试通过，4 项按上游设计忽略，0 项失败

## FastCtx 是什么

FastCtx 是一个使用 Rust 编写的纯本地 MCP 工具运行时，为 AI 编程智能体提供：

- 文件读取
- 内容搜索
- 文件查找
- 安全批量替换
- Bash 命令执行
- 持久后台任务管理

Codex 不再需要为常见仓库操作反复拼接 PowerShell、Bash、grep 或 find 命令。
FastCtx 统一处理路径、编码、目录遍历、并行搜索、分页和输出边界，让模型把更多
上下文用于理解代码和完成任务。

## Windows ARM64 v1 特性

- 原生 Windows ARM64 PE，可执行文件机器类型为 `AA64`
- 支持中文文件名和中文目录
- 支持 UTF-8、GBK、UTF-16、Shift-JIS、Big5 等文本编码
- 支持 PNG、JPEG、GIF、WebP 和 BMP 图片
- 支持任意文件的十六进制读取
- 支持 Scoop 安装的 Git Bash
- 579 项测试通过
- 4 项上游正常忽略
- 0 项测试失败
- 提供 ZIP、构建信息、验证记录和 SHA-256 校验

当前 ARM64 构建未启用 PDF 功能。

## 快速开始

### 1. 下载

打开：

[FastCtx Windows ARM64 v1 Release](https://github.com/Simplepine/fastctx/releases/tag/v1)

下载以下两个文件：

- `fastctx-v0.2.1-windows-arm64-no-pdf-verified.zip`
- `fastctx-v0.2.1-windows-arm64-no-pdf-verified.zip.sha256`

也可以直接下载：

- [下载 verified ZIP](https://github.com/Simplepine/fastctx/releases/download/v1/fastctx-v0.2.1-windows-arm64-no-pdf-verified.zip)
- [下载 SHA-256 校验文件](https://github.com/Simplepine/fastctx/releases/download/v1/fastctx-v0.2.1-windows-arm64-no-pdf-verified.zip.sha256)

### 2. 校验文件

在下载目录打开 PowerShell：

```powershell
Get-FileHash `
  -Algorithm SHA256 `
  -LiteralPath .\fastctx-v0.2.1-windows-arm64-no-pdf-verified.zip

Get-Content `
  -LiteralPath .\fastctx-v0.2.1-windows-arm64-no-pdf-verified.zip.sha256
```

两处哈希值必须一致：

```text
7ABD2C7BA6286DAA19EFEBDF9E920BB063FC5015BDCD5A70CE8BFF4C9E55CC02
```

### 3. 解压

```powershell
Expand-Archive `
  -LiteralPath .\fastctx-v0.2.1-windows-arm64-no-pdf-verified.zip `
  -DestinationPath C:\Tools\FastCtx
```

最终程序路径为：

```text
C:\Tools\FastCtx\fastctx-v0.2.1-windows-arm64-no-pdf-verified\fastctx.exe
```

### 4. 检查版本

```powershell
& 'C:\Tools\FastCtx\fastctx-v0.2.1-windows-arm64-no-pdf-verified\fastctx.exe' --version
```

预期输出：

```text
fastctx 0.2.1
```

如果 Windows 提示缺少 `VCRUNTIME140.dll`，请安装最新版 Microsoft Visual C++
Redistributable ARM64。

## 接入 Codex

### 方式一：使用 FastCtx 控制终端

运行：

```powershell
& 'C:\Tools\FastCtx\fastctx-v0.2.1-windows-arm64-no-pdf-verified\fastctx.exe'
```

进入全屏控制终端后：

1. 检查输出档位和工具设置。
2. 根据需要启用 Bash terminal。
3. 打开 Apply 页面。
4. 确认配置变更。
5. 选择 **Apply**。
6. 完全重启 Codex。

Apply 会把二进制复制到稳定目录：

```text
~/.fastctx/bin/fastctx.exe
```

完成后，即使删除最初解压的下载目录，Codex 中的 FastCtx 仍可继续使用。

### 方式二：手动配置

编辑：

```text
%USERPROFILE%\.codex\config.toml
```

添加：

```toml
[mcp_servers.fastctx]
command = "C:/Tools/FastCtx/fastctx-v0.2.1-windows-arm64-no-pdf-verified/fastctx.exe"
args = ["serve"]
startup_timeout_sec = 120
default_tools_approval_mode = "writes"

[features.code_mode]
direct_only_tool_namespaces = ["mcp__fastctx"]
```

保存后完全重启 Codex。

默认会出现四个工具：

```text
read
grep
glob
replace
```

`default_tools_approval_mode = "writes"` 表示读取和搜索可以直接执行，文件替换及
命令执行需要确认。

## 如何使用

FastCtx 接入后不需要手工编写 MCP JSON。直接用自然语言告诉 Codex 要做什么。

### 读取文件

```text
读取 C:/work/my-project/src/main.rs 的前 200 行。
```

```text
读取 C:/资料/中文项目/说明.txt。
```

```text
用 GBK 编码读取 C:/旧项目/配置.txt。
```

### 搜索内容

```text
在 C:/work/my-project 中搜索所有 TODO，返回匹配内容和前后一行。
```

```text
在 src 目录中搜索所有调用 old_api 的 Rust 文件。
```

### 查找文件

```text
查找 C:/work/my-project 下所有 **/*.toml 文件。
```

```text
列出最近修改的 20 个 TypeScript 文件。
```

### 安全替换

```text
预览把 src 目录中的 old_name 替换成 new_name，不要真正写入。
```

确认预览无误后：

```text
执行刚才的替换，最多允许修改 30 处。
```

### 运行命令

启用 Bash 工具后：

```text
在 C:/work/my-project 运行 cargo test。
```

```text
后台启动 npm run dev，然后持续查看输出。
```

FastCtx 的读取和搜索结果会明确标记：

- `Complete`：结果完整
- `Partial`：结果分页，需要按提示继续

出现 `Partial` 时，应使用结果末尾给出的精确参数继续读取。

## 工具说明

FastCtx 最多提供九个同级 MCP 工具。

| 工具 | 默认状态 | 用途 |
|---|---:|---|
| `read` | 开启 | 读取文本、图片和十六进制原始数据 |
| `grep` | 开启 | 使用 Rust 正则表达式搜索文件内容 |
| `glob` | 开启 | 按路径模式查找文件 |
| `replace` | 开启 | 预览或执行机械批量替换 |
| `run` | 关闭 | 前台执行 Bash 命令 |
| `run_background` | 关闭 | 启动持久后台任务 |
| `job_output` | 关闭 | 增量读取后台任务输出 |
| `job_kill` | 关闭 | 终止任务及其完整进程树 |
| `job_list` | 关闭 | 列出运行中或已保存的任务 |

## `read`

`read` 支持：

- 带 1 基行号的文本输出
- `offset` 和 `limit` 分页
- UTF-8 和 BOM
- GBK、Shift-JIS、Big5、UTF-16 等编码
- PNG、JPEG、GIF、WebP 和 BMP
- 任意文件的分页 hex 视图

编码存在歧义时，FastCtx 会返回候选编码和重试方式，不会直接输出乱码。

本 ARM64 构建不支持 PDF。

## `grep`

`grep` 使用 ripgrep 同源的 Rust 搜索组件，支持：

- 单文件和目录树搜索
- `.gitignore` 和 `.ignore`
- 隐藏文件
- glob 和文件类型筛选
- 大小写控制
- 多行正则表达式
- 匹配上下文
- 并行搜索
- 请求取消
- 稳定分页

无法可靠识别编码或在搜索期间发生变化的文件会被明确列入跳过报告。

## `glob`

`glob` 用于按路径模式查找文件，例如：

```text
**/*.rs
src/**/*.ts
**/*.{toml,json}
```

支持按路径排序、按修改时间排序、分页以及项目忽略规则。

## `replace`

`replace` 适合确定性的机械修改，例如：

- 符号重命名
- import 改写
- 配置键迁移
- 固定模式删除

安全机制包括：

- `dry_run` 预览
- `max_replacements` 限制影响范围
- 写入前冻结候选集
- 并发修改检查
- 同目录原子替换
- 保留原编码和 BOM
- 保留 CRLF 或 LF
- 保留未修改字节

需要理解代码语义的编辑仍应由 Codex 的 `apply_patch` 完成。

## 启用 Bash 工具

Windows 需要 Git Bash。可以安装 Git for Windows，也可以使用 Scoop：

```powershell
scoop install git
```

本社区版支持探测：

```text
%USERPROFILE%\scoop\apps\git\current\usr\bin\bash.exe
```

手动配置时，将参数改为：

```toml
args = ["serve", "--enable-shell"]
```

保存后重启 Codex。

Bash 工具拥有执行本地命令的能力，因此默认关闭。

## 后台任务

`run_background` 创建的任务由独立监督进程管理：

- Codex 会话关闭后仍可继续运行
- MCP Server 重启后可以重新找回
- 输出和状态保存在 `~/.fastctx/jobs/`
- 可以增量读取输出
- 可以终止整个进程树
- 已完成记录按存储配额回收

适合构建、测试、开发服务器和其他长时间任务。

## 安全与隐私

FastCtx 是纯本地工具：

- MCP 工具调用本身不发送遥测
- 文件内容不会上传给 FastCtx 服务
- 后台任务记录只保存在当前用户目录
- FastCtx Server 继承启动它的 Windows 用户权限
- Bash 默认关闭
- `replace` 支持预览和原子写入

FastCtx MCP Server 位于 Codex 自身工具沙箱之外。它能访问的文件范围由 FastCtx
进程的 Windows 用户权限决定。

如果希望所有工具都逐次确认，可以使用：

```toml
[mcp_servers.fastctx]
default_tools_approval_mode = "prompt"
```

## 控制终端

直接运行 `fastctx.exe` 会打开全屏控制终端，可管理：

- MCP 配置
- 输出 token 档位
- 搜索 CPU 上限
- Bash 工具
- 后台任务并发
- 任务存储配额
- Jobs dashboard
- Apply 和 Unapply
- 更新检查
- 17 种界面语言

## 卸载

### 使用 Apply 安装

运行：

```powershell
& 'C:\Tools\FastCtx\fastctx-v0.2.1-windows-arm64-no-pdf-verified\fastctx.exe' unapply --yes
```

然后重启 Codex。

### 手动配置

从 `%USERPROFILE%\.codex\config.toml` 中删除：

- `[mcp_servers.fastctx]`
- `direct_only_tool_namespaces` 中的 `mcp__fastctx`

重启 Codex 后删除解压目录。

## 当前限制

- PDF 功能未启用。
- 可执行文件没有 Authenticode 签名。
- Windows ARM64 npm 包尚不存在。
- npm 自动安装和 npm 自动替换暂不支持 ARM64。
- 社区版可能落后于上游版本。
- 上游尚未正式把 Windows ARM64 加入发布矩阵。
- GitHub `v1` 是社区发布编号，不代表上游 FastCtx 1.0。

## 从源码构建

需要：

- Windows ARM64
- Visual Studio Build Tools
- HostARM64/ARM64 MSVC
- Windows SDK
- Rust `aarch64-pc-windows-msvc`

在 ARM64 Developer Command Prompt 中：

```powershell
rustup toolchain install stable-aarch64-pc-windows-msvc
rustup default stable-aarch64-pc-windows-msvc

cargo +stable build `
  --locked `
  --release `
  --no-default-features `
  --target aarch64-pc-windows-msvc
```

生成文件：

```text
target\aarch64-pc-windows-msvc\release\fastctx.exe
```

完整编译检查：

```powershell
cargo +stable check `
  --locked `
  --no-default-features `
  --all-targets `
  --target aarch64-pc-windows-msvc
```

## 技术结构

项目使用 Rust 2024 Edition，主要模块包括：

| 模块 | 作用 |
|---|---|
| `src/read_tool/` | 文本、图片、PDF 和 hex 读取 |
| `src/grep_tool.rs` | 内容搜索 |
| `src/glob_tool.rs` | 文件遍历与 glob |
| `src/edit/` | 替换、文件锁和原子写入 |
| `src/shell/` | Bash 与持久后台任务 |
| `src/tui/` | 全屏控制终端 |
| `src/update/` | 更新检查、下载与回滚 |
| `src/control/` | Apply、Unapply 和配置所有权 |
| `src/encoding/` | 编码检测与转换 |
| `src/server.rs` | MCP 工具注册 |
| `src/stdio_transport.rs` | stdio MCP 传输 |

主要依赖包括 Tokio、rmcp、Rayon、ripgrep 搜索组件、encoding_rs、Ratatui 和
Windows Job Objects。

## 验证结果

Windows ARM64 `v1` 已完成：

- Rust 格式检查
- 锁定依赖的全目标编译检查
- Release 全测试目标编译
- 579 项测试通过
- 4 项正常忽略
- 0 项失败
- MCP 初始化与工具列表验证
- 中文路径 UTF-8 文本读取
- 中文路径 PNG 读取
- 中文路径 GBK 读取
- grep 和 glob 中文目录遍历
- replace dry-run 不修改源文件
- PE `AA64` 验证
- ZIP 下载后 SHA-256 复验

## 许可证

FastCtx 使用双许可证：

```text
MIT OR Apache-2.0
```

再分发时请保留：

- `LICENSE-MIT`
- `LICENSE-APACHE`
- `NOTICE`
- `THIRD_PARTY_LICENSES.md`

## 致谢

FastCtx 由 [yc-duan](https://github.com/yc-duan) 创建和维护。

本 fork 只提供：

- Windows ARM64 构建验证
- Scoop Git Bash 兼容
- 中文路径回归测试
- 社区二进制分发

通用功能、架构和绝大部分代码均属于原项目成果。欢迎关注、Star 和支持：

[https://github.com/yc-duan/fastctx](https://github.com/yc-duan/fastctx)

## 上游文档

- [上游中文文档](./README.zh-CN.md)
- [上游英文文档](https://github.com/yc-duan/fastctx/blob/main/README.md)
