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
        description = "Run a shell command with bash (Git Bash on Windows; system bash elsewhere)\nand return its merged stdout+stderr with the exit code. Write POSIX bash —\nnever PowerShell. Commands must be non-interactive: there is no TTY or\nstdin; use flags like -y or --no-edit. A non-zero exit code is a normal\nresult, not an error. Oversized output is truncated (with a note); to get\nthe full output, redirect it to a file (command > file 2>&1) and page that\nfile with the read tool. Default timeout 120000 ms (max 240000) — start\nanything longer with run_background. cwd must be absolute when given.\nIf the output looks garbled (U+FFFD), pass encoding with the source\nencoding label (e.g. \"gbk\"). The last line states Complete or Partial.",
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
        description = "Start a bash command as a background job and return its job_id\nimmediately. Use for builds, tests, servers, or anything that may exceed\ntwo minutes. Jobs run independently of this session: they survive server\nand Codex restarts, and their output and exit code stay retrievable by\njob_id afterwards. Check on it with job_output; stop with job_kill;\nrediscover past jobs with job_list. There is no timeout — a job runs\nuntil it exits or is killed. Everything the job prints is also kept in a\nplain log file whose path is returned here: read or grep it with the read\ntool for anything job_output does not show.",
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
        description = "Query a background job: its status (running, exited with its code, or\ninterrupted) plus the newest output you have not been shown yet. wait_ms\nis how long this query may take (0-60000, default 30000): it returns as\nsoon as the job reaches a terminal state, and otherwise waits the window\nout — intermediate output does not end the wait. Pass wait_ms=0 for an\nimmediate snapshot. Long output is windowed: you get the newest lines that\nfit, plus the start of the log on the first call, and a note naming the\nexact lines that were skipped. Nothing is lost — the job's whole output is\na plain log file on disk, and its line numbers are the seq numbers used\nhere, so read or grep that path for anything not shown. Works for jobs\nstarted in earlier sessions. If output looks garbled (U+FFFD), call again\nwith encoding set to the source encoding (e.g. \"gbk\") — stored bytes are\nre-decoded losslessly. Complete appears only once the job ends; a job that\nnever exits never reports it, so take what you need and do other work\ninstead of polling.",
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
