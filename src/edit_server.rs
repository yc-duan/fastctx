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
        description = "Batch find-and-replace across a file or directory (Rust regex, same engine\nas grep; no lookaround). A reference to an undefined capture group is\nrejected before any write. To delete whole lines, include \\n in the\npattern. Matching is leftmost-first and non-overlapping; unlike grep,\n`^`/`$` anchor the whole file by default — use (?m) for per-line anchors.\nRespects .gitignore; skips .git and binaries; files whose encoding cannot\nbe determined are skipped and listed. Each file is written atomically with\na concurrent-modification check, preserving its original encoding, BOM, and\nline endings.",
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
