// What commands printed, in the deeds specimen (`deeds.ts`), as a model
// finding its way in the World prints it: a batch sent through one `on`,
// the World's own commands asked how they are used, its help alone and
// long, a check that failed, and a command that printed nothing.

import { run, type Script } from "./script";
import type { Speaker } from "../world/events";
import { stepped } from "./deeds-world";

const HELP = [
  "Commands for working on this machine, and on the user's other machines when there are any.",
  "",
  "Usage: fastctx [OPTIONS] [COMMAND]",
  "",
  "Commands:",
  "  mcp        The account's MCP servers: list, add, import, test and remove",
  "  profile    Read or change the account's instructions and host settings",
  "  plan       Read or replace this conversation's plan",
  "  sense      Leave a sentinel to wait for something",
  "  ui         Open the World in this terminal",
  "  node       Run this machine's node in the foreground",
  "  renew      Replace this machine's node and guardian using a local executable",
  "  service    Install, uninstall or inspect a node/hub service on this machine",
  "  uninstall  Preview or execute removal of the World installed on this computer",
  "  server     Install a verified release on a Linux server and join a World",
  "  serve      Serve the World's tools to an agent session over MCP's stdio",
  "  view       Look at files: text with line numbers, images as images, a PDF's text by page",
  "  edit       Edit a file using SEARCH/REPLACE blocks from standard input",
  "  write      Create or replace a file with the text from standard input",
  "  w          See what runs and who works now, and what happened since your last turn",
  "  on         Run part of a script on another of the user's machines",
  "  stop       Stop jobs and everything they started, on whichever machine each runs",
  "  wait       Wait until jobs end, then show how each ended and its last lines",
  "  nodes      List the user's machines: which are online and what each is like",
  "  copy       Copy files and directories on one of the user's machines or between them",
  "  talk       Talk to a resident on any machine, or open a session to talk to",
  "  project    The user's projects: list, show and enter them",
  "  page       A project's pages: HTML the user opens in the World app",
  "  login      Log this computer in to an account on a hub",
  "  logout     Log this computer out of an account",
  "  token      Codes that let other devices into an account",
  "  accounts   The hubs this computer is on, with their accounts and machines",
  "  share      Share one of an account's machines with another account",
  "  update     Bring this machine's World program up to the newest release",
  "  apply      Give the sessions you open on this machine the World's tools",
  "  help       Print this message or the help of the given subcommand(s)",
  "",
  "Options:",
  "      --licenses  Print this executable's licenses and third-party notices",
  "  -h, --help      Print help",
  "  -V, --version   Print version",
].join("\n");

export function printoutWork(s: Script, user: Speaker): void {
  s.heard("t9", user, "先熟悉一下 fastctx：看看有哪些机器、在做什么，再试试几个命令的用法。");
  s.started("t9");
  s.say("t9", "thinking", "**Looking around before doing anything**", 2);
  // One batch through one `on`: nothing of it recognised, one output.
  s.tool(
    "t9",
    stepped(
      run(
        "on laptop <<'RUN'\nfastctx nodes\nfastctx w\nfastctx project list\nfastctx plan\npwd\nuname -a\nRUN",
        "=== run on laptop (exited 0) ===",
        [
          "=== nodes: laptop ===",
          "laptop  this computer  linux x86_64 · bash 5.2.21 · home /home/me · fastctx 1.0.1-beta.12",
          "",
          "--- hub home (connected for 35m) · working for me; registered under me",
          "gpu  online  linux x86_64 · 4× A100",
          "lab  online  linux x86_64",
          "=== w: all projects (laptop online) ===",
          "working",
          "  codex-ba8a (you)  laptop  Codex  turn 5s: run on laptop <<'RUN' …",
          "=== projects on laptop: mark50 ===",
          "mark50  /srv/mark50",
          "No plan yet.",
          "/srv/mark50",
          "Linux laptop 6.8.0-45-generic #45-Ubuntu SMP PREEMPT_DYNAMIC x86_64 GNU/Linux",
        ].join("\n"),
        { duration_ms: 900 },
      ),
      null,
    ),
    1,
  );
  // The help of a few of the World's commands, a name given a value
  // between: three commands that print into one stream.
  s.tool(
    "t9",
    stepped(
      run(
        "on laptop <<'RUN'\nfastctx view --help\nfastctx sense --help\nscratch_dir=\"$FASTCTX_JOB_TEMP/probe\"\nls \"$scratch_dir\"\nRUN",
        "=== run on laptop (exited 0) ===",
        [
          "Look at files: text with line numbers, images as images, a PDF's text by page",
          "",
          "Usage: fastctx view [OPTIONS] <PATHS>...",
          "",
          "Leave a sentinel to wait for something",
          "",
          "Usage: fastctx sense [OPTIONS] <CONDITION>",
          "probe.txt",
        ].join("\n"),
        { duration_ms: 700 },
      ),
      null,
    ),
    1,
  );
  s.tool("t9", run("fastctx --help", "=== run on laptop (exited 0) ===", HELP, { duration_ms: 400 }), 1);
  s.tool(
    "t9",
    run(
      "python scripts/check_offsets.py --strict",
      "=== run on laptop (exited 2) ===",
      ["checking 412 offsets in /srv/ingest/data", "offset 7312: replayed batch starts at the committed row", "error: 1 offset out of order (expected 7313, found 7312)"].join("\n"),
      { duration_ms: 1_800 },
    ),
    1,
  );
  s.tool(
    "t9",
    run(
      "python scripts/replay.py --dry-run --from 7312",
      "=== run on laptop (exited 0) ===",
      [
        "replaying from offset 7312 (dry run)",
        ...Array.from({ length: 18 }, (_, at) => `  batch ${7312 + at}  ${(at * 37) % 200 + 40} rows  ${at % 5 === 4 ? "skipped: already committed" : "would write"}`),
        "dry run: 18 batches, 15 would be written, 3 already committed",
      ].join("\n"),
      { duration_ms: 2_600 },
    ),
    1,
  );
  s.tool("t9", run("mkdir -p /srv/ingest/out", "=== run on laptop (exited 0) ===", "", { duration_ms: 100 }), 1);
  s.say("t9", "message", "看过了：三台机器都在线，mark50 是唯一的项目，还没有计划。偏移检查在 7312 处报错，我下一步去修重放的起点。", 2);
  s.ended("t9", "completed", null, 1);
}
