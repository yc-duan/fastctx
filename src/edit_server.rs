//! The default byte-preserving replacement route in the single `fastctx` server.

use crate::edit::ReplaceRequest;
use crate::server::FastCtxServer;
use crate::server_support::run_blocking;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use std::sync::Arc;

#[tool_router(router = edit_tool_router, vis = "pub(crate)")]
impl FastCtxServer {
    #[tool(
        name = "replace",
        description = "Batch find-and-replace across a file or directory (Rust regex, same engine\nas grep; no lookaround). replacement supports $1/${name} groups, $$ for a\nliteral $; a reference to an undefined capture group is rejected before any\nwrite; an empty replacement deletes the match (include \\n in the\npattern to delete whole lines). Matching is leftmost-first and\nnon-overlapping; unlike grep, `^`/`$` anchor the whole file by default —\nuse (?m) for per-line anchors. Respects .gitignore; skips .git, binaries, and files\nwhose encoding cannot be determined (listed, never silent). Each file is\nwritten atomically with a concurrent-modification check, preserving its\noriginal encoding, BOM, and line endings. path is required. Set\ndry_run=true to preview; set max_replacements to cap the blast radius. The\nlast line states Complete or Partial.",
        annotations(
            title = "Batch replace file contents",
            read_only_hint = false,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    async fn replace(&self, Parameters(request): Parameters<ReplaceRequest>) -> CallToolResult {
        let replace = self.replace.clone();
        run_blocking(Arc::clone(&self.replace_permits), move || {
            replace.replace(request)
        })
        .await
    }
}
