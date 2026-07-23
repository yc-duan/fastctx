//! Unified rmcp registration, feature gating, and shared tool state.

use crate::budget::{GLOB_TOKEN_BUDGET_ENV, GREP_TOKEN_BUDGET_ENV};
use crate::edit::ReplaceService;
use crate::file_executor::GrepGlobExecutor;
use crate::glob_tool::{GlobRequest, glob_files_cancellable};
use crate::grep_tool::{GrepRequest, grep_files_cancellable};
use crate::read_tool::{ReadRequest, read_file};
use crate::server_manifest::{ToolContract, ToolManifest};
use crate::server_support::{run_blocking, run_blocking_cancellable};
use crate::shell::FastShell;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, Implementation, ServerCapabilities, ServerInfo};
use rmcp::service::RequestContext;
use rmcp::{RoleServer, ServerHandler, tool, tool_handler, tool_router};
use std::sync::Arc;
use tokio::sync::Semaphore;

const MAX_FILE_OPERATIONS: usize = 8;
const MAX_SHELL_OPERATIONS: usize = 16;
const MAX_REPLACE_OPERATIONS: usize = 8;

/// Optional tool groups published by the single `fastctx` server.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ServerOptions {
    /// Publish the five shell tools.
    pub enable_shell: bool,
}

impl ServerOptions {
    /// Enables all nine tools; intended for contract tests and doctor probes.
    pub const fn all() -> Self {
        Self { enable_shell: true }
    }
}

/// The single stateful MCP server for default file tools and the optional shell group.
#[derive(Clone, Debug)]
pub struct FastCtxServer {
    tool_router: ToolRouter<Self>,
    options: ServerOptions,
    pub(crate) shell: FastShell,
    pub(crate) replace: ReplaceService,
    pub(crate) file_permits: Arc<Semaphore>,
    pub(crate) grep_glob_executor: Arc<GrepGlobExecutor>,
    pub(crate) shell_permits: Arc<Semaphore>,
    pub(crate) replace_permits: Arc<Semaphore>,
}

impl FastCtxServer {
    /// Creates the default four-tool server, including byte-preserving replacement.
    pub fn new() -> Self {
        Self::with_options(ServerOptions::default())
    }

    /// Creates one server whose visible tools are selected by startup flags.
    pub fn with_options(options: ServerOptions) -> Self {
        Self::with_options_and_executor(options, GrepGlobExecutor::shared())
    }

    /// Creates a server with the process-startup search executor selected by current-user config.
    pub(crate) fn with_options_and_executor(
        options: ServerOptions,
        grep_glob_executor: Arc<GrepGlobExecutor>,
    ) -> Self {
        let mut tool_router = Self::file_tool_router();
        tool_router.merge(Self::shell_tool_router());
        tool_router.merge(Self::edit_tool_router());
        for entry in ToolManifest::entries() {
            if !entry.group.enabled(options.enable_shell) {
                tool_router.remove_route(entry.name);
            }
        }
        let definitions = tool_router.list_all();
        ToolManifest::validate(&definitions, options.enable_shell)
            .expect("the compiled tool router must match ToolManifest");
        Self {
            tool_router,
            options,
            shell: FastShell::new(),
            replace: ReplaceService::new(),
            file_permits: Arc::new(Semaphore::new(MAX_FILE_OPERATIONS)),
            grep_glob_executor,
            shell_permits: Arc::new(Semaphore::new(MAX_SHELL_OPERATIONS)),
            replace_permits: Arc::new(Semaphore::new(MAX_REPLACE_OPERATIONS)),
        }
    }

    /// Returns every definition exposed by MCP `tools/list` for tests and diagnostics.
    pub fn tool_definitions(&self) -> Vec<rmcp::model::Tool> {
        self.tool_router.list_all()
    }

    /// Returns stable contract hashes for every currently published tool.
    pub fn tool_contracts(&self) -> Vec<ToolContract> {
        ToolManifest::contracts(&self.tool_definitions())
            .expect("validated server tools must have manifest entries")
    }

    /// Returns the startup feature selection used by this server.
    pub const fn options(&self) -> ServerOptions {
        self.options
    }
}

impl Default for FastCtxServer {
    fn default() -> Self {
        Self::new()
    }
}

#[tool_router(router = file_tool_router, vis = "pub(crate)")]
impl FastCtxServer {
    #[tool(
        name = "read",
        description = "Read a file (text, image, or PDF) from the local filesystem. Text returns\n1-based `N<tab>content` lines, 2000 per page; page with offset/limit. Images\n(PNG/JPG/GIF/WebP/BMP) are shown to you visually. PDFs return the selected\npages' text layer (pdf_mode=\"text\", default) or each page rendered as an\nimage (pdf_mode=\"image\"). Text mode requires `pages` over 10 pages; image\nmode defaults to 4 pages. Max 20 pages per call. view=\"hex\" dumps any file's\nraw bytes. Text output is always UTF-8; omit encoding for conservative\nauto-detection (BOM and valid UTF-8 are trusted, legacy text only after\nconsistency checks) — if uncertain it returns an error listing candidate\nencodings instead of guessed text, so pass encoding (e.g. \"gbk\") only when\nyou know the source encoding or a prior read reported ambiguity. file_path must\nbe absolute. Text, PDF, and hex responses end with a Complete or Partial status\n— continue only with the exact parameters a Partial note provides. Plain\nimages, warnings, and errors are self-contained.",
        annotations(
            title = "Read local file",
            read_only_hint = true,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    async fn read(&self, Parameters(request): Parameters<ReadRequest>) -> CallToolResult {
        run_blocking(Arc::clone(&self.file_permits), move || read_file(request)).await
    }

    #[tool(
        name = "grep",
        description = "Fast regex content search (ripgrep engine; Rust regex, no lookaround). Output\nmodes: \"files_with_matches\" (default, paths only), \"content\" (matching lines,\noptional context), \"count\" (per-file occurrence counts — total matches, not\nmatching-line count), \"summary\" (global totals only).\nRespects .gitignore; searches hidden files; skips .git and binaries. Files are\ndecoded to UTF-8 before searching; files whose encoding can't be determined, or\nthat change during a directory search, are skipped and listed (never silently) —\npass fallback_encoding (directory) or encoding (single file) to resolve encoding;\na changing single-file target returns an error. Matching is line-by-line: `^` and\n`$` anchor line boundaries and are CRLF-aware. Set multiline=true for patterns\nspanning lines (`.` matches newlines; `\\n` also matches `\\r\\n`). A path component\nof the form ~fastctx~b...~ (reversible bytes/UTF-8) or ~fastctx~w...~ (Windows\nUTF-16) is a filename escape; copy that whole component verbatim in later calls\nand do not decode or rewrite it. The last line of every successful result states\nComplete or Partial — continue only with the exact offset a Partial note provides;\nerrors are self-contained.",
        annotations(
            title = "Search file contents",
            read_only_hint = true,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    async fn grep(
        &self,
        Parameters(request): Parameters<GrepRequest>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        run_blocking_cancellable(
            context.id,
            context.ct,
            Arc::clone(&self.file_permits),
            Arc::clone(&self.grep_glob_executor),
            GREP_TOKEN_BUDGET_ENV,
            move |operation, executor| grep_files_cancellable(operation, executor, request),
        )
        .await
    }

    #[tool(
        name = "glob",
        description = "Find files by glob pattern, e.g. \"**/*.rs\" or \"src/**/*.ts\". Returns absolute\npaths sorted by path (or newest first with sort=\"modified\"), 100 per page.\nfilter_mode \"project\" (default) respects .gitignore and skips .git;\nfilter_mode \"all\" lists everything. Omit `path` to search the session working\ndirectory — omit the field entirely, never pass \"null\" or \"undefined\". A path\ncomponent of the form ~fastctx~b...~ (reversible bytes/UTF-8) or ~fastctx~w...~\n(Windows UTF-16) is a filename escape; copy that whole component verbatim in\nlater calls and do not decode or rewrite it. The last line of every successful\nresult states Complete or Partial — continue only with the exact offset a Partial\nnote provides; errors are self-contained.",
        annotations(
            title = "Match file paths",
            read_only_hint = true,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    async fn glob(
        &self,
        Parameters(request): Parameters<GlobRequest>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        run_blocking_cancellable(
            context.id,
            context.ct,
            Arc::clone(&self.file_permits),
            Arc::clone(&self.grep_glob_executor),
            GLOB_TOKEN_BUDGET_ENV,
            move |operation, executor| glob_files_cancellable(operation, executor, request),
        )
        .await
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for FastCtxServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(
                env!("CARGO_PKG_NAME"),
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(if self.options.enable_shell {
                "Use read, grep, and glob for inspection, replace for mechanical file edits, and the POSIX-bash shell tools for terminal work."
            } else {
                "Use read, grep, and glob for inspection, and replace for mechanical file edits."
            })
    }
}

#[cfg(test)]
mod tests {
    use super::{FastCtxServer, ServerOptions};
    use crate::file_executor::GrepGlobExecutor;
    use crate::search_parallelism::MAX_SEARCH_PARALLELISM;
    use std::sync::Arc;

    #[test]
    fn configured_executor_is_the_server_search_source_for_serial_mid_and_maximum_p() {
        let middle = (MAX_SEARCH_PARALLELISM / 2).max(1);
        for parallelism in [1, middle, MAX_SEARCH_PARALLELISM] {
            let executor = Arc::new(GrepGlobExecutor::with_test_parallelism(parallelism));
            let server = FastCtxServer::with_options_and_executor(
                ServerOptions::default(),
                Arc::clone(&executor),
            );
            assert!(Arc::ptr_eq(&server.grep_glob_executor, &executor));
            assert_eq!(server.grep_glob_executor.parallelism(), parallelism);
            assert_eq!(server.grep_glob_executor.extra_capacity(), parallelism - 1);
        }
    }
}
