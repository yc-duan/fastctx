//! Unified rmcp registration, feature gating, and shared tool state.

use crate::budget::{GLOB_TOKEN_BUDGET_ENV, GREP_TOKEN_BUDGET_ENV, READ_TOKEN_BUDGET_ENV};
use crate::edit::ReplaceService;
use crate::file_executor::GrepGlobExecutor;
use crate::glob_tool::{GlobRequest, glob_files_cancellable};
use crate::grep_tool::{GrepRequest, grep_files_cancellable};
use crate::read_tool::{ReadRequest, read_file};
use crate::server_manifest::{ToolContract, ToolManifest};
use crate::server_support::{
    BudgetRetry, CancellableBlockingRequest, run_blocking, run_blocking_cancellable,
};
use crate::session::SessionContext;
use crate::shell::FastShell;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolResult, ErrorCode, ErrorData, Implementation, ListResourceTemplatesResult,
    ListResourcesResult, Meta, PaginatedRequestParams, ReadResourceRequestParams,
    ReadResourceResult, ResourceContents, ResourceTemplate, ServerCapabilities, ServerInfo,
};
use rmcp::service::RequestContext;
use rmcp::{RoleServer, ServerHandler, tool, tool_handler, tool_router};
use std::sync::Arc;
use tokio::sync::Semaphore;

const MAX_FILE_OPERATIONS: usize = 8;
const MAX_SHELL_OPERATIONS: usize = 16;
const MAX_REPLACE_OPERATIONS: usize = 8;

/// Optional tool groups published by the single `fastctx` server.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
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
    pub(crate) session: Arc<SessionContext>,
    pub(crate) activity: Arc<crate::runtime::activity::RuntimeActivity>,
}

/// Expensive executors and process-wide admission gates shared by every control-center session.
#[derive(Clone, Debug)]
pub struct SharedRuntime {
    file_permits: Arc<Semaphore>,
    grep_glob_executor: Arc<GrepGlobExecutor>,
    shell_permits: Arc<Semaphore>,
    replace: ReplaceService,
    replace_permits: Arc<Semaphore>,
    activity: Arc<crate::runtime::activity::RuntimeActivity>,
}

impl SharedRuntime {
    /// Creates one per-user runtime around the configured search executor.
    pub(crate) fn new(grep_glob_executor: Arc<GrepGlobExecutor>) -> Arc<Self> {
        Self::with_activity(
            grep_glob_executor,
            crate::runtime::activity::RuntimeActivity::new(),
        )
    }

    pub(crate) fn with_activity(
        grep_glob_executor: Arc<GrepGlobExecutor>,
        activity: Arc<crate::runtime::activity::RuntimeActivity>,
    ) -> Arc<Self> {
        Arc::new(Self {
            file_permits: Arc::new(Semaphore::new(MAX_FILE_OPERATIONS)),
            grep_glob_executor,
            shell_permits: Arc::new(Semaphore::new(MAX_SHELL_OPERATIONS)),
            replace: ReplaceService::new(),
            replace_permits: Arc::new(Semaphore::new(MAX_REPLACE_OPERATIONS)),
            activity,
        })
    }
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
        Self::with_session_and_runtime(
            options,
            SessionContext::library_default(),
            SharedRuntime::new(grep_glob_executor),
        )
    }

    /// Creates one isolated MCP connection backed by a shared per-user runtime.
    pub(crate) fn with_session_and_runtime(
        options: ServerOptions,
        session: Arc<SessionContext>,
        runtime: Arc<SharedRuntime>,
    ) -> Self {
        let mut tool_router = Self::file_tool_router();
        tool_router.merge(Self::shell_tool_router());
        tool_router.merge(Self::edit_tool_router());
        // rmcp's attribute accepts only a literal. Replace its inert placeholder before the router
        // is observable so the positive route still has one production source.
        tool_router
            .map
            .get_mut("inspect_local_file")
            .expect("the compiled file router must contain inspect_local_file")
            .attr
            .description = Some(crate::model_guidance::inspect_tool_description().into());
        for entry in ToolManifest::entries() {
            if !entry.group.enabled(options.enable_shell) {
                tool_router.remove_route(entry.name);
            }
        }
        // Every consumer of a tool definition reads it back out of this router — the
        // stdio `tools/list` answer, `tool_definitions`, and the contract hashes doctor
        // compares across processes. Normalizing here keeps all of them on one shape;
        // doing it in `tool_definitions` instead would make doctor compare a normalized
        // expectation against an underived wire answer.
        for route in tool_router.map.values_mut() {
            route.attr.input_schema = Arc::new(crate::tool_schema::normalize_published_schema(
                &route.attr.input_schema,
            ));
        }
        let definitions = tool_router.list_all();
        ToolManifest::validate(&definitions, options.enable_shell)
            .expect("the compiled tool router must match ToolManifest");
        Self {
            tool_router,
            options,
            shell: FastShell::with_session(Arc::clone(&session)),
            replace: runtime.replace.clone(),
            file_permits: Arc::clone(&runtime.file_permits),
            grep_glob_executor: Arc::clone(&runtime.grep_glob_executor),
            shell_permits: Arc::clone(&runtime.shell_permits),
            replace_permits: Arc::clone(&runtime.replace_permits),
            activity: Arc::clone(&runtime.activity),
            session,
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
        name = "inspect_local_file",
        description = "Inspect local files.",
        annotations(
            title = "Inspect local file",
            read_only_hint = true,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    async fn inspect_local_file(
        &self,
        Parameters(request): Parameters<ReadRequest>,
    ) -> CallToolResult {
        let _activity = self.activity.request();
        let status_shell = self.shell.clone();
        run_blocking(
            Arc::clone(&self.session),
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
        description = "Fast regex content search (ripgrep engine; Rust regex, no lookaround). Output\nmodes: \"files_with_matches\" (default, paths only), \"content\", \"count\" (total\nmatches, not matching lines), \"summary\" (global totals). Respects .gitignore;\nsearches hidden files; skips .git and binaries. Files are decoded to UTF-8\nbefore searching; files whose encoding can't be determined, that change, or\nthat cannot be searched are skipped and listed for directory targets; the\nequivalent single-file failure returns an error. Matching is line-by-line:\n`^` and `$` anchor line boundaries and are CRLF-aware. A path component of the\nform ~fastctx~b...~ (reversible bytes/UTF-8) or ~fastctx~w...~ (Windows UTF-16)\nis a filename escape; copy that whole component verbatim in later calls and\ndo not decode or rewrite it. The last line of every successful result states\nComplete or Partial — continue only with the exact offset a Partial note\nprovides; errors are self-contained.",
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
        let _activity = self.activity.request();
        run_blocking_cancellable(
            CancellableBlockingRequest::new(
                Arc::clone(&self.session),
                context.id,
                context.ct,
                Arc::clone(&self.file_permits),
                Arc::clone(&self.grep_glob_executor),
                GREP_TOKEN_BUDGET_ENV,
            ),
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
        description = "Find files by glob pattern, e.g. \"**/*.rs\" or \"src/**/*.ts\". Matches files\nonly, never directories. Returns absolute\npaths sorted by path (or newest first with sort=\"modified\"), 100 per page by\ndefault. filter_mode defaults to \"project\" (respects .gitignore, skips .git);\n\"all\" lists everything. Omit `path` entirely for the session working directory\n— never pass \"null\" or \"undefined\". A path component of the form ~fastctx~b...~\n(reversible bytes/UTF-8) or ~fastctx~w...~ (Windows UTF-16) is a filename\nescape; copy that whole component verbatim in later calls and do not decode or\nrewrite it. The last line of every successful result states Complete or Partial\n— continue only with the exact offset a Partial note provides; errors are\nself-contained.",
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
        let _activity = self.activity.request();
        run_blocking_cancellable(
            CancellableBlockingRequest::new(
                Arc::clone(&self.session),
                context.id,
                context.ct,
                Arc::clone(&self.file_permits),
                Arc::clone(&self.grep_glob_executor),
                GLOB_TOKEN_BUDGET_ENV,
            ),
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
        self.activity.touch();
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .build(),
        )
            .with_server_info(Implementation::new(
                env!("CARGO_PKG_NAME"),
                env!("CARGO_PKG_VERSION"),
            ))
            // 2026-07-24: hosts render these instructions as the tool namespace's one-line
            // blurb and may keep only its first line and first 250 characters, so this text
            // has to introduce the toolset within that budget. Behavioural rules belong in
            // the host guidance file, which has no such limit.
            .with_instructions(crate::model_guidance::server_instructions(
                self.options.enable_shell,
            ))
    }

    // FastCtx is a tool-only MCP server by design: it publishes `inspect_local_file`,
    // `grep`, `glob`, `replace`, and the optional shell group — not MCP resources.
    //
    // However, some MCP hosts (notably Codex Desktop / CLI) inject a generic
    // `read_mcp_resource` tool for *every* configured server regardless of whether
    // the server advertises the `resources` capability. When the model sees that tool
    // but has no valid server name or URI template to fill it with, it fabricates
    // placeholders like `server="?"`, `uri="?"` — producing a chain of failed calls
    // that never reach the server (tracked in #18, #26).
    //
    // The 0.2.2 approach (override `resources/*` to reject uniformly) backfired:
    // a *failure* makes the model retry with a different invented `server` argument
    // rather than switch to the correct `inspect_local_file` tool — users reported
    // chains of invented server names.
    //
    // This patch takes the opposite approach: **make resources genuinely work** so
    // the model has a real target instead of a failure to retry against.
    //
    // 1. `list_resource_templates` returns a `file:///{path}` template with a
    //    description that teaches the model the absolute-path format. This gives the
    //    model a valid URI pattern to fill, eliminating the `?` placeholder.
    //
    // 2. `read_resource` accepts a `file:///` URI (or a plain absolute path) and
    //    reads the file by reusing the existing `read_file` core, so encoding
    //    detection, token budgets, PDF/image handling, and error messages stay
    //    consistent with the `inspect_local_file` tool.
    //
    // 3. `list_resources` stays on the rmcp default (empty list) because FastCtx
    //    does not manage a dynamic resource collection — the template is the single
    //    entry point.
    //
    // This is a complementary approach to PR #21 (which handles misrouted
    // `resources/read` but keeps `resources/templates/list` rejected). By also
    // publishing a template, we address the root cause: the model had no valid URI
    // to fill in the first place.

    /// Returns one `file:///{path}` template so the model has a valid URI pattern
    /// for `read_mcp_resource` instead of fabricating `?` placeholders.
    fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, ErrorData> {
        self.activity.touch();
        let template = ResourceTemplate::new(
            "file:///{path}",
            "Read a local file by absolute path. Replace {path} with the full \
             filesystem path (e.g. file:///C:/Users/me/code.rs on Windows, \
             file:///home/me/code.rs on Unix). Returns text, images, or PDF \
             content depending on the file type.",
        );
        Ok(ListResourceTemplatesResult::with_all_items(vec![template]))
    }

    /// Reads a local file from a `file:///` URI or a plain absolute path.
    ///
    /// This handles the misrouted `resources/read` calls that some hosts produce
    /// even when the server does not advertise the `resources` capability. Remote
    /// authorities, non-file schemes, queries, and fragments are rejected.
    ///
    /// **Token budget protection**: this goes through `run_blocking` with
    /// `READ_TOKEN_BUDGET_ENV`, the same path as `inspect_local_file`. This
    /// ensures the response respects the configured output budget and triggers
    /// the guarded-burst stub when the response would exhaust the turn's shared
    /// output pool — preventing context-window blowout via `resources/read`
    /// (#24). Without this, a large file read through `resources/read` would
    /// bypass the budget and dump unlimited content into the model's context.
    fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, ErrorData> {
        self.activity.touch();
        let uri = request.uri;

        // Parse the URI or plain path into a local filesystem path.
        let path = crate::paths::parse_local_path_input(&uri)
            .map_err(|message| ErrorData::invalid_params(message, None))?;

        let file_path = path.to_str().map(str::to_owned).ok_or_else(|| {
            ErrorData::invalid_params(
                "The local resource path cannot be represented as UTF-8.",
                None,
            )
        })?;

        // Reuse the same session, permits, and budget system as inspect_local_file.
        // run_blocking acquires a file-permit, activates the session, applies the
        // guarded-burst budget ceiling, and converts to CallToolResult. We then
        // convert CallToolResult back to ReadResourceResult.
        let session = Arc::clone(&self.session);
        let permits = Arc::clone(&self.file_permits);
        let shell = self.shell.clone();
        let result = run_blocking(
            session,
            permits,
            READ_TOKEN_BUDGET_ENV,
            move || shell.background_status(None),
            BudgetRetry::Safe,
            move || {
                read_file(ReadRequest {
                    file_path: Some(file_path),
                    files: None,
                    offset: None,
                    limit: None,
                    pages: None,
                    pdf_mode: None,
                    encoding: None,
                    view: None,
                })
            },
        )
        .await;

        call_tool_result_to_resource_result(uri, result)
    }
}

/// Converts a `CallToolResult` from `run_blocking` into an MCP `ReadResourceResult`.
///
/// Text content blocks become `ResourceContents::text` and image blocks become
/// `ResourceContents::blob` with the appropriate MIME type. When the tool
/// response is an error, it is surfaced as an `invalid_params` protocol error
/// so the host can display a meaningful message rather than an opaque failure.
fn call_tool_result_to_resource_result(
    uri: String,
    result: CallToolResult,
) -> Result<ReadResourceResult, ErrorData> {
    use rmcp::model::ContentBlock;

    if result.is_error == Some(true) {
        let message = result
            .content
            .into_iter()
            .filter_map(|block| match block {
                ContentBlock::Text(text) => Some(text.text),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        return Err(ErrorData::invalid_params(
            if message.is_empty() {
                "The local resource could not be read.".to_string()
            } else {
                message
            },
            None,
        ));
    }

    let contents = result
        .content
        .into_iter()
        .map(|block| match block {
            ContentBlock::Text(text) => {
                ResourceContents::text(text.text, uri.clone())
            }
            ContentBlock::Image(image) => {
                let contents = ResourceContents::blob(image.data, uri.clone())
                    .with_mime_type(image.mime_type);
                if image
                    ._meta
                    .as_ref()
                    .and_then(|meta| meta.0.get("codex/imageDetail"))
                    .is_some_and(|v| v == "high")
                {
                    let mut meta = Meta::new();
                    meta.0.insert(
                        "codex/imageDetail".to_string(),
                        serde_json::Value::String("high".to_string()),
                    );
                    contents.with_meta(meta)
                } else {
                    contents
                }
            }
            _ => ResourceContents::text(
                "Unsupported content block type in resource read.".to_string(),
                uri.clone(),
            ),
        })
        .collect();

    Ok(ReadResourceResult::new(contents))
}

#[cfg(test)]
mod tests {
    use super::{FastCtxServer, ServerOptions, SharedRuntime, call_tool_result_to_resource_result};
    use crate::file_executor::GrepGlobExecutor;
    use crate::search_parallelism::MAX_SEARCH_PARALLELISM;
    use rmcp::model::{CallToolResult, ContentBlock, ErrorCode, ImageContent, Meta};
    use std::sync::Arc;

    #[test]
    fn call_tool_result_to_resource_preserves_text_and_image() {
        let mut image_meta = Meta::new();
        image_meta.0.insert(
            "codex/imageDetail".to_string(),
            serde_json::Value::String("high".to_string()),
        );
        let result = call_tool_result_to_resource_result(
            "file:///C:/notes.txt".to_string(),
            CallToolResult::success(vec![
                ContentBlock::Text(rmcp::model::TextContent::new("1\tline")),
                ContentBlock::Image(
                    ImageContent::new("aW1hZ2U=".to_string(), "image/png".to_string())
                        .with_meta(image_meta),
                ),
            ]),
        )
        .unwrap();
        let value = serde_json::to_value(&result).unwrap();
        assert_eq!(value["contents"][0]["uri"], "file:///C:/notes.txt");
        assert_eq!(value["contents"][0]["mimeType"], "text/plain");
        assert_eq!(value["contents"][0]["text"], "1\tline");
        assert_eq!(value["contents"][1]["uri"], "file:///C:/notes.txt");
        assert_eq!(value["contents"][1]["mimeType"], "image/png");
        assert_eq!(value["contents"][1]["blob"], "aW1hZ2U=");
        assert_eq!(value["contents"][1]["_meta"]["codex/imageDetail"], "high");
    }

    #[test]
    fn call_tool_result_to_resource_surfaces_errors() {
        let error = call_tool_result_to_resource_result(
            "file:///C:/missing.txt".to_string(),
            CallToolResult::error(vec![ContentBlock::Text(
                rmcp::model::TextContent::new("File does not exist: C:/missing.txt"),
            )]),
        )
        .unwrap_err();
        assert_eq!(error.code, ErrorCode::INVALID_PARAMS);
        assert_eq!(error.message, "File does not exist: C:/missing.txt");
        assert!(error.data.is_none());
    }

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

    #[test]
    fn connections_share_runtime_resources_but_keep_distinct_session_contexts() {
        let executor = Arc::new(GrepGlobExecutor::with_test_parallelism(1));
        let runtime = SharedRuntime::new(Arc::clone(&executor));
        let first = FastCtxServer::with_session_and_runtime(
            ServerOptions::all(),
            crate::session::SessionContext::library_default(),
            Arc::clone(&runtime),
        );
        let second = FastCtxServer::with_session_and_runtime(
            ServerOptions::all(),
            crate::session::SessionContext::library_default(),
            runtime,
        );

        assert!(Arc::ptr_eq(&first.grep_glob_executor, &executor));
        assert!(Arc::ptr_eq(
            &first.grep_glob_executor,
            &second.grep_glob_executor
        ));
        assert!(Arc::ptr_eq(&first.file_permits, &second.file_permits));
        assert!(Arc::ptr_eq(&first.shell_permits, &second.shell_permits));
        assert!(Arc::ptr_eq(&first.replace_permits, &second.replace_permits));
        assert!(first.replace.shares_locks_with(&second.replace));
        assert!(Arc::ptr_eq(&first.activity, &second.activity));
        assert!(!Arc::ptr_eq(&first.session, &second.session));
    }
}
