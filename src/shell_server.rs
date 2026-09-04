//! Shell-tool routes merged into the single `fastctx` server.

use crate::budget::{GLOBAL_TOKEN_BUDGET_ENV, JOB_OUTPUT_TOKEN_BUDGET_ENV, RUN_TOKEN_BUDGET_ENV};
use crate::server::FastCtxServer;
use crate::server_support::{BudgetRetry, run_blocking};
use crate::shell::{
    JobKillRequest, JobListRequest, JobOutputRequest, RunBackgroundRequest, RunRequest,
};
use rmcp::RoleServer;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::service::RequestContext;
use rmcp::{tool, tool_router};
use std::sync::Arc;

#[tool_router(router = shell_tool_router, vis = "pub(crate)")]
impl FastCtxServer {
    #[tool(
        name = "run",
        description = "Use for non-interactive local CLI work, including Git, build/test tools,\npackage managers, database CLIs, and project scripts. Commands run with bash\n(Git Bash on Windows; system bash elsewhere) and return merged stdout+stderr\nwith the exit code. Write POSIX bash — never PowerShell. There is no TTY or\nstdin; use flags like -y or --no-edit. A non-zero exit code is a normal\nresult, not an error. Oversized output is windowed; for the full output,\nredirect it to a file (command > file 2>&1) and inspect that file.\nDefault timeout 120000 ms, ceiling 240000 — start anything that may outlast\nit with run_background.",
        annotations(
            title = "Run bash command",
            read_only_hint = false,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    async fn run(
        &self,
        Parameters(request): Parameters<RunRequest>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        self.local_run(request, context).await
    }

    #[tool(
        name = "run_background",
        // rmcp accepts only a literal here. `FastCtxServer::with_session_and_runtime` replaces
        // this inert placeholder with the enabled-set-aware text before the router is observable.
        description = "Start a bash command as a background job.",
        annotations(
            title = "Start background bash job",
            read_only_hint = false,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    async fn run_background(
        &self,
        Parameters(request): Parameters<RunBackgroundRequest>,
    ) -> CallToolResult {
        let _activity = self.activity.request();
        let shell = self.shell.clone();
        let status_shell = self.shell.clone();
        run_blocking(
            Arc::clone(&self.session),
            Arc::clone(&self.shell_permits),
            GLOBAL_TOKEN_BUDGET_ENV,
            move || status_shell.background_status(None),
            BudgetRetry::Never,
            move || shell.run_background(request.clone()),
        )
        .await
    }

    #[tool(
        name = "job_output",
        // rmcp accepts only a literal here. `FastCtxServer::with_session_and_runtime` replaces
        // this inert placeholder with the enabled-set-aware text before the router is observable.
        description = "Query a background job.",
        annotations(
            title = "Check background job output",
            read_only_hint = true,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    async fn job_output(
        &self,
        Parameters(request): Parameters<JobOutputRequest>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        let _activity = self.activity.request();
        let shell = self.shell.clone();
        let status_shell = self.shell.clone();
        let excluded_job = request.job_id.clone();
        run_blocking(
            Arc::clone(&self.session),
            Arc::clone(&self.shell_permits),
            JOB_OUTPUT_TOKEN_BUDGET_ENV,
            move || status_shell.background_status(Some(&excluded_job)),
            BudgetRetry::Never,
            move || shell.job_output_until_cancelled(request.clone(), || context.ct.is_cancelled()),
        )
        .await
    }

    #[tool(
        name = "job_kill",
        description = "Kill a background job's whole process tree. Killing a job that has\nalready exited is not an error.",
        annotations(
            title = "Kill background job",
            read_only_hint = false,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    async fn job_kill(&self, Parameters(request): Parameters<JobKillRequest>) -> CallToolResult {
        let _activity = self.activity.request();
        let shell = self.shell.clone();
        let status_shell = self.shell.clone();
        let excluded_job = request.job_id.clone();
        run_blocking(
            Arc::clone(&self.session),
            Arc::clone(&self.shell_permits),
            GLOBAL_TOKEN_BUDGET_ENV,
            move || status_shell.background_status(Some(&excluded_job)),
            BudgetRetry::Never,
            move || shell.job_kill(request.clone()),
        )
        .await
    }

    #[tool(
        name = "job_list",
        description = "List background jobs across all FastCtx sessions for the current user. Use\nstatus=\"all\" only when both lifecycles are needed. Results are newest first\nwithin each lifecycle. Finished records remain available until the job\nstorage limit evicts the oldest. Continue a paged result with offset equal to\nthe last covered job number.",
        annotations(
            title = "List background jobs",
            read_only_hint = true,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    async fn job_list(&self, Parameters(request): Parameters<JobListRequest>) -> CallToolResult {
        let _activity = self.activity.request();
        let shell = self.shell.clone();
        let status_shell = self.shell.clone();
        run_blocking(
            Arc::clone(&self.session),
            Arc::clone(&self.shell_permits),
            GLOBAL_TOKEN_BUDGET_ENV,
            move || status_shell.background_status(None),
            BudgetRetry::Never,
            move || shell.job_list(request.clone()),
        )
        .await
    }
}

impl FastCtxServer {
    /// The local `run`, shared by the 1.0 route and the World route without `node`.
    pub(crate) async fn local_run(
        &self,
        request: RunRequest,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        let _activity = self.activity.request();
        let shell = self.shell.clone();
        let status_shell = self.shell.clone();
        run_blocking(
            Arc::clone(&self.session),
            Arc::clone(&self.shell_permits),
            RUN_TOKEN_BUDGET_ENV,
            move || status_shell.background_status(None),
            BudgetRetry::Never,
            move || shell.run_until_cancelled(request.clone(), || context.ct.is_cancelled()),
        )
        .await
    }
}
