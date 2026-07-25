//! Unified rmcp registration, feature gating, and shared tool state.

use crate::budget::{GLOB_TOKEN_BUDGET_ENV, GREP_TOKEN_BUDGET_ENV, READ_TOKEN_BUDGET_ENV};
use crate::edit::ReplaceService;
use crate::file_executor::GrepGlobExecutor;
use crate::glob_tool::{GlobRequest, glob_files_cancellable};
use crate::grep_tool::{GrepRequest, grep_files_cancellable};
use crate::read_tool::{ReadRequest, read_file};
use crate::server_manifest::{ToolContract, ToolManifest};
use crate::server_support::{BudgetRetry, run_blocking, run_blocking_cancellable};
use crate::shell::FastShell;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolResult, ErrorCode, ErrorData, Implementation, ListResourceTemplatesResult,
    ListResourcesResult, PaginatedRequestParams, ReadResourceRequestParams, ReadResourceResult,
    ServerCapabilities, ServerInfo,
};
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
        description = "Read one file (text, image, or PDF) or a batch of text files from the local\nfilesystem. Paths must be absolute. Text returns 1-based `N<tab>content`\nlines, 2000 per page; page with offset/limit. For several text files in one\ncall, pass files=[{\"path\": ...}, ...] instead of file_path: one token\nbudget, per-file problems reported inline without failing the batch, and a\nPartial note returns the exact files array for the next call. Images\n(PNG/JPG/GIF/WebP/BMP) are shown to you visually. PDFs return the selected\npages' text layer or those pages rendered as images; image mode defaults to\n4 pages. view=\"hex\" dumps any file's raw bytes. PDFs, images, and hex view\nare single-file only. Text output is always UTF-8; when auto-detection is\nnot confident it returns an error listing candidate encodings instead of\nguessed text, so pass encoding only then. Text, PDF, and hex responses end\nwith a Complete or Partial status — continue only with the exact parameters\na Partial note provides.",
        annotations(
            title = "Read local file",
            read_only_hint = true,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    async fn read(&self, Parameters(request): Parameters<ReadRequest>) -> CallToolResult {
        let status_shell = self.shell.clone();
        run_blocking(
            Arc::clone(&self.file_permits),
            READ_TOKEN_BUDGET_ENV,
            move || status_shell.background_status(None),
            BudgetRetry::Safe,
            move || read_file(request.clone()),
        )
        .await
    }

    #[tool(
        name = "grep",
        description = "Fast regex content search (ripgrep engine; Rust regex, no lookaround). Output\nmodes: \"files_with_matches\" (default, paths only), \"content\", \"count\" (total\nmatches, not matching lines), \"summary\" (global totals). Respects .gitignore;\nsearches hidden files; skips .git and binaries. Files are decoded to UTF-8\nbefore searching; files whose encoding can't be determined, or that change\nduring a directory search, are skipped and listed; a changing single-file\ntarget returns an error. Matching is line-by-line: `^` and `$` anchor line\nboundaries and are CRLF-aware. A path component of the form ~fastctx~b...~\n(reversible bytes/UTF-8) or ~fastctx~w...~ (Windows UTF-16) is a filename\nescape; copy that whole component verbatim in later calls and do not decode\nor rewrite it. The last line of every successful result states Complete or\nPartial — continue only with the exact offset a Partial note provides; errors\nare self-contained.",
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
            {
                let shell = self.shell.clone();
                move || shell.background_status(None)
            },
            move |operation, executor| grep_files_cancellable(operation, executor, request.clone()),
        )
        .await
    }

    #[tool(
        name = "glob",
        description = "Find files by glob pattern, e.g. \"**/*.rs\" or \"src/**/*.ts\". Returns absolute\npaths sorted by path (or newest first with sort=\"modified\"), 100 per page by\ndefault. filter_mode defaults to \"project\" (respects .gitignore, skips .git);\n\"all\" lists everything. Omit `path` entirely for the session working directory\n— never pass \"null\" or \"undefined\". A path component of the form ~fastctx~b...~\n(reversible bytes/UTF-8) or ~fastctx~w...~ (Windows UTF-16) is a filename\nescape; copy that whole component verbatim in later calls and do not decode or\nrewrite it. The last line of every successful result states Complete or Partial\n— continue only with the exact offset a Partial note provides; errors are\nself-contained.",
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
            {
                let shell = self.shell.clone();
                move || shell.background_status(None)
            },
            move |operation, executor| glob_files_cancellable(operation, executor, request.clone()),
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
            // 2026-07-24: hosts render these instructions as the tool namespace's one-line
            // blurb and may keep only its first line and first 250 characters, so this text
            // has to introduce the toolset within that budget. Behavioural rules belong in
            // the host guidance file, which has no such limit.
            .with_instructions(if self.options.enable_shell {
                "Local-file tools: read (one file or a batch), grep (content search), glob (find paths), replace (mechanical find-and-replace), plus POSIX-bash shell tools. Pass absolute paths, never file:// URIs. FastCtx publishes tools, not MCP resources."
            } else {
                "Local-file tools: read (one file or a batch), grep (content search), glob (find paths), and replace (mechanical find-and-replace). Pass absolute paths, never file:// URIs. FastCtx publishes tools, not MCP resources."
            })
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        Err(not_a_resource_server())
    }

    async fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, ErrorData> {
        Err(not_a_resource_server())
    }

    async fn read_resource(
        &self,
        _request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, ErrorData> {
        Err(not_a_resource_server())
    }
}

/// Rejection shared by every `resources/*` method, naming the tool that actually does the job.
///
/// 2026-07-24: hosts publish generic resource tools for every configured MCP server without
/// checking whether the server declared the `resources` capability, so these methods are reachable
/// even though FastCtx never advertises them. The SDK default would answer the two list methods
/// with an empty array, which reads as "this server does resources and happens to have none" and
/// invites a follow-up read of an invented URI; one rejection for all three cuts that off at the
/// first step. Callers arrive holding a `file://` URL, so the recovery names the parameter shape
/// as well as the tool.
fn not_a_resource_server() -> ErrorData {
    ErrorData::new(
        ErrorCode::METHOD_NOT_FOUND,
        "Use mcp__fastctx__read with an absolute file path (not a file:// URI) to read local files, and mcp__fastctx__glob to list paths. FastCtx publishes tools, not MCP resources.",
        None,
    )
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
