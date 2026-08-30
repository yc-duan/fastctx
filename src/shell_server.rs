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
        description = "Use for non-interactive local CLI work, including Git, build/test tools,\npackage managers, database CLIs, and project scripts. Commands run with bash\n(Git Bash on Windows; system bash elsewhere) and return merged stdout+stderr\nwith the exit code. Write POSIX bash — never PowerShell. There is no TTY or\nstdin; use flags like -y or --no-edit. A non-zero exit code is a normal\nresult, not an error. Oversized output is windowed; for the full output,\nredirect it to a file (command > file 2>&1) and inspect that file.\nDefault timeout 120000 ms, ceiling 240000 — start anything that may outlast\nit with run_background. If output looks garbled (U+FFFD), pass encoding\n(e.g. \"gbk\").",
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

    #[tool(
        name = "run_background",
        description = "Start a bash command as a background job and return its job_id\nimmediately. Use for builds, tests, servers, or anything that may outlast\nrun's four-minute maximum. Jobs survive server and agent restarts; their\noutput and exit code stay retrievable by job_id. Check on it with\njob_output; stop with job_kill; rediscover past jobs with job_list. There\nis no timeout: a job runs until it exits or is killed. Everything it\nprints is kept in a plain log file whose path is returned here;\ninspect_local_file or grep that path for anything job_output does not show.\nBackground status refreshes only when another FastCtx tool returns; it is not\na push notification, so keep working and check back when useful.",
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
        description = "Query a background job: its status plus output after after_seq (or after\nthe session cursor when omitted). Works for jobs started in earlier sessions.\nLong output is windowed: the first lines on the initial call and newest lines\nthat fit. Sequence numbers in the head note map directly to the plain log file\non disk, so inspect_local_file or grep that path for omitted output. The call\nblocks up to wait_ms; use 0 for a snapshot, and raise it only when you have\nnothing else to do. If output looks garbled (U+FFFD), call again with encoding\nset to the source encoding (e.g. \"gbk\").",
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
        description = "List background jobs across all FastCtx sessions for the current user. Use\nstatus=\"all\" only when both lifecycles are needed. Results are newest first\nwithin each lifecycle. Finished records remain available until the job\nstorage limit evicts the oldest.",
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
