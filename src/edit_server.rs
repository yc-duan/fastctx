//! The default byte-preserving replacement route in the single `fastctx` server.

use crate::budget::GLOBAL_TOKEN_BUDGET_ENV;
use crate::control::settings;
use crate::edit::ReplaceRequest;
use crate::model::ToolResponse;
use crate::server::FastCtxServer;
use crate::server_support::{BudgetRetry, run_blocking};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use std::sync::Arc;

#[tool_router(router = edit_tool_router, vis = "pub(crate)")]
impl FastCtxServer {
    #[tool(
        name = "replace",
        // rmcp accepts only a literal here. `FastCtxServer::with_session_and_runtime` replaces
        // this inert placeholder with the enabled-set-aware text before the router is observable.
        description = "Batch find-and-replace across a file or directory.",
        annotations(
            title = "Batch replace file contents",
            read_only_hint = false,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    async fn replace(&self, Parameters(request): Parameters<ReplaceRequest>) -> CallToolResult {
        let _activity = self.activity.request();
        let replace = self.replace.clone();
        let control_paths = self.session.control_paths.clone();
        let status_shell = self.shell.clone();
        run_blocking(
            Arc::clone(&self.session),
            Arc::clone(&self.replace_permits),
            GLOBAL_TOKEN_BUDGET_ENV,
            move || status_shell.background_status(None),
            BudgetRetry::Never,
            move || {
                let max_file_size_mib = match settings::load(&control_paths)
                    .and_then(|settings| settings.replace_file_limit_mib())
                {
                    Ok(limit) => limit,
                    Err(error) => {
                        return ToolResponse::error(format!(
                            "Cannot use replace settings: {error}. Repair the FastCtx configuration and retry."
                        ));
                    }
                };
                replace.replace_with_limit(request.clone(), max_file_size_mib)
            },
        )
        .await
    }
}
