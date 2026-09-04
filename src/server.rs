//! Unified rmcp registration, feature gating, and shared tool state.

use crate::budget::{GLOB_TOKEN_BUDGET_ENV, GREP_TOKEN_BUDGET_ENV, READ_TOKEN_BUDGET_ENV};
use crate::edit::ReplaceService;
use crate::file_executor::GrepGlobExecutor;
use crate::glob_tool::{GlobRequest, glob_files_cancellable};
use crate::grep_tool::{GrepRequest, grep_files_cancellable};
use crate::read_tool::{ReadRequest, read_file};
use crate::server_manifest::{EnabledTools, ToolContract, ToolManifest};
use crate::server_support::{
    BudgetRetry, CancellableBlockingRequest, run_blocking, run_blocking_cancellable,
};
use crate::session::SessionContext;
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
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct ServerOptions {
    /// Validated tool names to publish.
    pub tools: EnabledTools,
    /// Publish the World-mode surface: this machine is enrolled in a World, so file tools and
    /// `run` take a `node` and the machine map is published. Decided by the proxy from the
    /// existence of `~/.fastctx/world.toml`, never from the network.
    #[serde(default)]
    pub world: bool,
}

impl ServerOptions {
    /// Enables all nine tools; intended for contract tests and doctor probes.
    pub const fn all() -> Self {
        Self {
            tools: EnabledTools::all(),
            world: false,
        }
    }

    /// The local, non-World surface for one enabled set.
    pub const fn local(tools: EnabledTools) -> Self {
        Self {
            tools,
            world: false,
        }
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
    /// The World client when this engine runs inside an enrolled node daemon; `None` on an
    /// unenrolled machine or in the degraded plain control center.
    pub(crate) world: Option<Arc<crate::world::client::WorldClient>>,
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
    world: Option<Arc<crate::world::client::WorldClient>>,
}

impl SharedRuntime {
    /// Creates one per-user runtime around the configured search executor.
    pub(crate) fn new(grep_glob_executor: Arc<GrepGlobExecutor>) -> Arc<Self> {
        Self::with_activity(
            grep_glob_executor,
            crate::runtime::activity::RuntimeActivity::new(),
            None,
        )
    }

    pub(crate) fn with_activity(
        grep_glob_executor: Arc<GrepGlobExecutor>,
        activity: Arc<crate::runtime::activity::RuntimeActivity>,
        world: Option<Arc<crate::world::client::WorldClient>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            file_permits: Arc::new(Semaphore::new(MAX_FILE_OPERATIONS)),
            grep_glob_executor,
            shell_permits: Arc::new(Semaphore::new(MAX_SHELL_OPERATIONS)),
            replace: ReplaceService::new(),
            replace_permits: Arc::new(Semaphore::new(MAX_REPLACE_OPERATIONS)),
            activity,
            world,
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
        // The job-log clause names the file tools that can read that log. Naming a tool this
        // target does not publish teaches the model a route that does not exist, and
        // co-occurrence beats negation, so the clause follows the enabled set (2026-08-30).
        for (name, description) in [
            (
                "run_background",
                crate::model_guidance::run_background_tool_description(options.tools),
            ),
            (
                "job_output",
                crate::model_guidance::job_output_tool_description(options.tools),
            ),
            (
                "replace",
                crate::model_guidance::replace_tool_description(options.tools),
            ),
        ] {
            tool_router
                .map
                .get_mut(name)
                .expect("the compiled router must contain every generated-description tool")
                .attr
                .description = Some(description.into());
        }
        if options.world {
            // World mode replaces the five node-taking tools with their World routes and adds
            // the machine map. Descriptions and schemas come from the local routes so the two
            // surfaces cannot drift apart; only the `node` property is added.
            let mut world_router = Self::world_tool_router();
            for (name, multi) in [
                ("inspect_local_file", true),
                ("grep", true),
                ("glob", true),
                ("replace", false),
                ("run", false),
            ] {
                let base = tool_router
                    .map
                    .get(name)
                    .expect("the compiled router must contain every node-taking tool");
                let description = base.attr.description.clone();
                let schema = crate::world::surface::add_node_property(
                    &base.attr.input_schema,
                    if multi {
                        crate::world::surface::NODE_PARAMETER_MULTI
                    } else {
                        crate::world::surface::NODE_PARAMETER_SINGLE
                    },
                );
                let route = world_router
                    .map
                    .get_mut(name)
                    .expect("the compiled World router must contain every node-taking tool");
                route.attr.description = description;
                route.attr.input_schema = Arc::new(schema);
                tool_router.remove_route(name);
            }
            tool_router.merge(world_router);
        }
        for entry in ToolManifest::entries() {
            if !options.tools.contains(entry.name) {
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
        ToolManifest::validate_options(&definitions, options)
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
            world: runtime.world.clone(),
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
        self.local_inspect_local_file(request).await
    }

    #[tool(
        name = "grep",
        description = "Fast regex content search (ripgrep engine; Rust regex, no lookaround). Output\nmodes: \"files_with_matches\" (default, paths only), \"content\", \"count\" (total\nmatches, not matching lines), \"summary\" (global totals). Respects .gitignore;\nsearches hidden files; skips .git and binaries. Files are decoded to UTF-8\nbefore searching; files whose encoding can't be determined, that change, or\nthat cannot be searched are skipped and listed for directory targets; the\nequivalent single-file failure returns an error. Matching is line-by-line:\n`^` and `$` anchor line boundaries and are CRLF-aware. A path component of the\nform ~fastctx~b...~ (reversible bytes/UTF-8) or ~fastctx~w...~ (Windows UTF-16)\nis a filename escape; copy that whole component verbatim in later calls and\ndo not decode or rewrite it. Continue a paged result with offset equal to the\nlast covered result number.",
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
        self.local_grep(request, context).await
    }

    #[tool(
        name = "glob",
        description = "Find files by glob pattern, e.g. \"**/*.rs\" or \"src/**/*.ts\". Matches files\nonly, never directories. Returns absolute paths sorted by path (or newest first\nwith sort=\"modified\"), 100 per page by default. filter_mode defaults to\n\"ignore\" (plain .ignore files only); \"all\" disables that filtering. Hidden\nfiles and .git are always visible unless a negative pattern excludes them.\noutput_mode defaults to \"paths\"; \"details\" returns compact JSON lines with\npath, byte size, and UTC modification time. Omit `path` entirely for the session\nworking directory — never pass \"null\" or \"undefined\". A path component of the\nform ~fastctx~b...~ (reversible bytes/UTF-8) or ~fastctx~w...~ (Windows UTF-16)\nis a filename escape; copy that whole component verbatim in later calls and do\nnot decode or rewrite it. Continue a paged result with offset equal to the last\ncovered file number.",
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
        self.local_glob(request, context).await
    }
}

/// The local execution of each file tool, shared by the 1.0 routes and the World routes
/// (which take the same path whenever `node` is absent).
impl FastCtxServer {
    pub(crate) async fn local_inspect_local_file(&self, request: ReadRequest) -> CallToolResult {
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

    pub(crate) async fn local_grep(
        &self,
        request: GrepRequest,
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

    pub(crate) async fn local_glob(
        &self,
        request: GlobRequest,
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
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(
                env!("CARGO_PKG_NAME"),
                env!("CARGO_PKG_VERSION"),
            ))
            // 2026-07-24: hosts render these instructions as the tool namespace's one-line
            // blurb and may keep only its first line and first 250 characters, so this text
            // has to introduce the toolset within that budget. Behavioural rules belong in
            // the host guidance file, which has no such limit.
            .with_instructions(crate::model_guidance::server_instructions(
                self.options.tools,
            ))
    }

    // The three `resources/*` methods stay on the rmcp defaults on purpose: both list methods
    // answer with an empty list, and `resources/read` answers method-not-found. Overriding them
    // to reject uniformly (added 0.2.2, reverted 2026-08-01) turned "this server has none" into a
    // failure, and a failed call makes a model retry with a different `server` argument rather
    // than switch tools — users reported chains of invented server names that the empty list
    // never produced. Do not reintroduce an override without evidence from a released build.
}

#[cfg(test)]
mod tests {
    use super::{FastCtxServer, ServerOptions, SharedRuntime};
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
