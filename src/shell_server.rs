//! Shell-tool routes merged into the single `fastctx` server.

use crate::server::FastCtxServer;
use crate::server_support::run_blocking;
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
        description = "Run a shell command with bash (Git Bash on Windows; system bash elsewhere)\nand return its merged stdout+stderr with the exit code. Write POSIX bash —\nnever PowerShell. Commands must be non-interactive: there is no TTY or\nstdin; use flags like -y or --no-edit. A non-zero exit code is a normal\nresult, not an error. Oversized output is truncated (with a note); to get\nthe full output, redirect it to a file (command > file 2>&1) and page that\nfile with the read tool. Default timeout 120000 ms (max 240000) — start\nanything longer with run_background. cwd must be absolute when given.\nThe last line states Complete or Partial.",
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
        let shell = self.shell.clone();
        run_blocking(Arc::clone(&self.shell_permits), move || {
            shell.run_until_cancelled(request, || context.ct.is_cancelled())
        })
        .await
    }

    #[tool(
        name = "run_background",
        description = "Start a bash command as a background job and return its job_id\nimmediately. Use for builds, tests, servers, or anything that may exceed\ntwo minutes. Jobs run independently of this session: they survive server\nand Codex restarts, and their output and exit code stay retrievable by\njob_id afterwards. Poll with job_output; stop with job_kill; rediscover\npast jobs with job_list. There is no timeout — a job runs until it exits\nor is killed.",
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
        let shell = self.shell.clone();
        run_blocking(Arc::clone(&self.shell_permits), move || {
            shell.run_background(request)
        })
        .await
    }

    #[tool(
        name = "job_output",
        description = "Return a background job's new output since the last call, plus its status\n(running, exited with its code, or interrupted). wait_ms long-polls: it\nreturns as soon as new output or the exit arrives, otherwise when the wait\nelapses. A Partial note gives an after_seq cursor — pass it back to resume\nidempotently if a call was lost. Works for jobs started in earlier\nsessions. Keep calling until the last line says Complete.",
        annotations(
            title = "Read background job output",
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
        let shell = self.shell.clone();
        run_blocking(Arc::clone(&self.shell_permits), move || {
            shell.job_output_until_cancelled(request, || context.ct.is_cancelled())
        })
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
        let shell = self.shell.clone();
        run_blocking(Arc::clone(&self.shell_permits), move || {
            shell.job_kill(request)
        })
        .await
    }

    #[tool(
        name = "job_list",
        description = "List background jobs across all FastCtx sessions for the current user.\nstatus defaults to running; use finished to inspect exited or interrupted\nrecords, or all only when both lifecycles are needed. Results are newest\nfirst within each lifecycle. limit defaults to the current-user\nfastshell.job_list_limit setting (20 initially, maximum 100), and offset\ncontinues a page. Finished records remain available until the job storage\nlimit evicts the oldest.",
        annotations(
            title = "List background jobs",
            read_only_hint = true,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    async fn job_list(&self, Parameters(request): Parameters<JobListRequest>) -> CallToolResult {
        let shell = self.shell.clone();
        run_blocking(Arc::clone(&self.shell_permits), move || {
            shell.job_list(request)
        })
        .await
    }
}
