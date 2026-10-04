// A world to look at the front page with: what `fastctx w` and `nodes`
// would answer on a desktop one morning, and what happened overnight.

import type { Address } from "../world/events";
import type { About, Action, MachineFacts, ProjectRecord, ResidentNow } from "../world/world";
import type { Away, WorldState } from "../home/model";
import type { AgentFacts, WorldAgents } from "../world/agents";
import type { WorldNow } from "../world/bridge";

const at = (time: string) => `2026-10-04T${time}Z`;

const claude = (version = "2.1.289"): About["host"] => ({ agent: "claude", client: "claude-code", version });
const codex = (): About["host"] => ({ agent: "codex", client: "codex-mcp-client", version: "0.160.0" });

function resident(
  id: string,
  machine: string,
  host: About["host"],
  town: string | null,
  model: string,
  born: string,
  state: Partial<ResidentNow> & { origin?: About["origin"] },
): ResidentNow {
  return {
    about: {
      id,
      host,
      machine,
      born_at: at(born),
      origin: state.origin ?? { kind: "user" },
      session: null,
      world_started: true,
      model,
    },
    town,
    here: state.here ?? true,
    now: state.now ?? null,
    last_action: state.last_action ?? null,
    moved: state.moved ?? null,
  };
}

const action = (time: string, machine: string, hand: string, what: string, result: string): Action => ({
  at: at(time),
  machine,
  hand,
  what,
  result,
});

const address = (machine: string, resident: string): Address => ({ machine, resident });

function facts(name: string, os: string, home: string): MachineFacts {
  return {
    name, os, arch: "x86_64", home, temp: os === "windows" ? `${home}/AppData/Local/Temp` : "/tmp",
    path_separator: os === "windows" ? "\\" : "/", case_sensitive: os !== "windows", version: "1.0.0",
  };
}

function record(name: string, what: string, machines: string[]): ProjectRecord {
  return { version: 1, name, what, machines, places: [], canon: null, by: { kind: "user" }, on: "desktop", at: at("00:00:00") };
}

export function specimenWorld(): { world: WorldState; now: Date; lastVisit: Date } {
  const world: WorldState = {
    here: "desktop",
    nodes: [
      { name: "desktop", online: true, since: at("01:10:00"), facts: { Ok: facts("desktop", "windows", "C:/Users/me") }, revoked: null },
      { name: "laptop", online: true, since: at("02:04:00"), facts: { Ok: facts("laptop", "windows", "C:/Users/lenovo") }, revoked: null },
      { name: "lab", online: true, since: at("00:00:00"), facts: { Ok: facts("lab", "linux", "/home/lenovo") }, revoked: null },
      { name: "hangzhou", online: true, since: at("00:00:00"), facts: { Ok: facts("hangzhou", "linux", "/root") }, revoked: null },
      { name: "studio", online: false, since: at("12:52:00"), facts: { Err: "offline" }, revoked: null },
      { name: "pixel", online: true, since: at("13:20:00"), facts: { Err: "a device" }, revoked: null, device: true },
    ],
    machines: [
      {
        kind: "answered",
        reply: {
          machine: "laptop",
          residents: [
            resident("claude-2b1c", "laptop", claude(), "mark50", "claude-opus-5-5", "05:02:00", {
              now: { owed: {}, answering: [], activity: "busy", since: at("13:36:00"), turn: "t3", trouble: null, desk: false, waiting_for_user: false },
              last_action: action("13:37:10", "lab", "run", "cd /srv/mark50 && git fetch laptop colleague/q16-edge && cargo test -p ctrl overflow_repro", "running"),
            }),
            resident("codex-91c0", "laptop", codex(), "mark50", "gpt-6.1-sol", "05:20:00", {
              origin: { kind: "colleague", asked_by: address("laptop", "claude-2b1c"), label: "造出让 Q32 积分溢出的输入" },
              here: false,
              now: { owed: {}, answering: [], activity: "asleep", since: at("13:34:00"), turn: null, trouble: null, desk: false, waiting_for_user: false },
              last_action: action("13:33:40", "laptop", "run", "cargo test -p ctrl overflow_repro", "exit 101"),
            }),
            resident("codex-7f3a", "laptop", codex(), "ingest", "gpt-6.1-sol", "08:40:00", {
              now: { owed: {}, answering: [], activity: "idle", since: at("09:31:00"), turn: null, trouble: null, desk: false, waiting_for_user: false },
              last_action: action("09:30:50", "laptop", "run", "cd /srv/ingest && cargo test 2>&1 | tail -3", "exit 0"),
            }),
          ],
          jobs: [],
        },
      },
      {
        kind: "answered",
        reply: {
          machine: "desktop",
          residents: [
            resident("codex-5e2a", "desktop", codex(), "ingest", "gpt-6.1-sol", "12:58:00", {
              now: { owed: {}, answering: [], activity: "busy", since: at("13:20:00"), turn: "t2", trouble: null, desk: false, waiting_for_user: true },
              last_action: action("13:21:30", "desktop", "apply_patch", "D:/work/ingest/deploy/compose.yaml", "1 file"),
            }),
          ],
          jobs: [],
        },
      },
      {
        kind: "answered",
        reply: {
          machine: "lab",
          residents: [
            resident("codex-0d1e", "lab", codex(), "mark50", "gpt-6.1-sol", "06:10:00", {
              origin: { kind: "colleague", asked_by: address("laptop", "claude-2b1c"), label: "训练关节模型 400 轮" },
              now: { owed: {}, answering: [], activity: "busy", since: at("13:12:00"), turn: "t5", trouble: { at: at("13:30:00"), text: "Reconnecting... 2/5" }, desk: false, waiting_for_user: false },
              last_action: action("13:31:00", "lab", "run", "tail -n 40 /data/mark50-sim/train.log", "exit 0"),
            }),
          ],
          jobs: [
            {
              job: "j-p0q1r2",
              machine: "lab",
              owner: address("lab", "codex-3c77"),
              started_at: at("07:18:00"),
              pid: 48121,
              command: "python train.py --epochs 400 --joint all",
              output: "/home/lenovo/.fastctx/world/towns/mark50/residents/codex-3c77/out/j-p0q1r2.log",
            },
          ],
        },
      },
      { kind: "answered", reply: { machine: "hangzhou", residents: [], jobs: [] } },
      { kind: "offline", machine: "studio", since: at("12:52:00") },
    ],
    towns: [
      { town: "mark50", record: record("mark50 研发", "为 Mark 50 战衣研发纳米单元的控制固件，并在真机上验证。", ["desktop", "laptop", "lab"]), journals: 7 },
      { town: "ingest", record: record("ingest 服务", "把传感器日志批量写进时序库的服务，跑在 lab 上。", ["laptop", "desktop", "lab"]), journals: 2 },
    ],
    away: {
      unasked: [{ machine: "studio", offline: true, why: null }],
      trouble: null,
      happenings: [
      { kind: "machine", at: at("12:52:00"), machine: "studio", online: false },
      { kind: "turn_ended", at: at("13:34:00"), who: address("laptop", "codex-91c0"), town: "mark50", how: "completed", reason: null, said: "Found one. With ki = 0x7FFF_0000 and a constant error of 32767 at dt = 1, narrow() wraps instead of saturating." },
      { kind: "journal", at: at("13:10:00"), town: "mark50", journal: "q16-fixed-point", entry: "20261004T131000Z-laptop", by: { kind: "resident", address: address("laptop", "claude-2b1c") }, first: "To see why step_response settled at 0.9612 on lab, measured the integral's rounding at Ki = 0.8, dt = 1 ms." },
      { kind: "job_ended", at: at("13:09:12"), job: "j-k3x9a2", machine: "lab", owner: address("laptop", "claude-2b1c"), town: "mark50", how: "exited 0" },
      { kind: "asked", at: at("13:09:40"), who: address("laptop", "claude-2b1c"), colleague: address("laptop", "codex-91c0"), town: "mark50", label: "造出让 Q32 积分溢出的输入" },
      { kind: "turn_ended", at: at("09:31:00"), who: address("laptop", "codex-7f3a"), town: "ingest", how: "completed", reason: null, said: "Done. The writer now commits every 5 000 rows and at end of file, and a retry replays only the rows since the last commit." },
      { kind: "turn_ended", at: at("08:52:00"), who: address("laptop", "codex-7f3a"), town: "ingest", how: "failed", reason: "stream disconnected before completion", said: null },
      { kind: "journal", at: at("07:40:00"), town: "ingest", journal: "batch-commit", entry: "20261004T074000Z-laptop", by: { kind: "resident", address: address("laptop", "codex-7f3a") }, first: "Writer opened one transaction per file; a 2.1 GB file held it for 41 min on 2026-10-03 (log attached)." },
      { kind: "job_ended", at: at("06:05:00"), job: "j-m8v2t0", machine: "lab", owner: address("lab", "codex-0d1e"), town: "mark50", how: "exited 1" },
      ],
    },
  };
  return { world, now: new Date(at("13:40:00")), lastVisit: new Date(at("04:30:00")) };
}

/** The same morning as the shell answers it: what `world_now` returns. */
export function specimenNow(): { world: WorldNow; now: Date; lastVisit: Date; away: Away | null } {
  const { world, now, lastVisit } = specimenWorld();
  return {
    world: {
      here: world.here,
      who: { machine: world.here, scope: { kind: "world", towns: world.towns }, machines: world.machines },
      nodes: {
        machine: world.here,
        world: "w-5be1fcee3751dc7a",
        link: { kind: "online", since: at("01:10:02"), hub: "121.40.82.28:7443", route: { kind: "card", name: "以太网 2" } },
        machines: world.nodes,
      },
      towns: world.towns,
    },
    now,
    lastVisit,
    away: world.away,
  };
}


/** What the programs on a specimen machine say of themselves: Codex from
 *  the desktop app with its models, Claude Code logged in; on lab, Claude
 *  Code installed but not logged in. */
export function specimenAgents(machine: string): WorldAgents {
  const efforts = (names: string[]) => names.map((effort) => ({ effort, description: "" }));
  const codex: AgentFacts = {
    agent: "codex",
    program: { kind: "known", value: { path: "C:/Program Files/WindowsApps/OpenAI.Codex/app/codex.exe", source: "desktop_app", exists: true } },
    version: { kind: "known", value: "codex-cli 0.160.0" },
    launch: { kind: "known", value: { kind: "codex_app_server", program: "codex.exe", pid: 4120 } },
    login: { kind: "known", value: { kind: "logged_in", method: "ChatGPT" } },
    models: {
      kind: "known",
      value: [
        { id: "gpt-6.1-sol", model: "gpt-6.1-sol", display_name: "GPT-6.1 Sol", description: "", is_default: true, default_reasoning_effort: "medium", reasoning_efforts: efforts(["low", "medium", "high", "xhigh", "max", "ultra"]) },
        { id: "gpt-6-astra", model: "gpt-6-astra", display_name: "GPT-6 Astra", description: "", is_default: false, default_reasoning_effort: "high", reasoning_efforts: efforts(["low", "medium", "high", "xhigh", "max", "ultra"]) },
        { id: "gpt-6-luna", model: "gpt-6-luna", display_name: "GPT-6 Luna", description: "", is_default: false, default_reasoning_effort: "medium", reasoning_efforts: efforts(["low", "medium", "high", "xhigh", "max"]) },
        { id: "gpt-5.5", model: "gpt-5.5", display_name: "GPT-5.5", description: "", is_default: false, default_reasoning_effort: "medium", reasoning_efforts: efforts(["low", "medium", "high", "xhigh"]) },
      ],
    },
    integration: { kind: "known", value: "in" },
    timings: [],
  };
  const claude: AgentFacts = {
    agent: "claude",
    program: { kind: "known", value: { path: "C:/Users/lenovo/.local/bin/claude.exe", source: "path", exists: true } },
    version: { kind: "known", value: "2.1.289 (Claude Code)" },
    launch: { kind: "known", value: { kind: "claude_command" } },
    login: { kind: "known", value: machine === "lab" ? { kind: "logged_out" } : { kind: "logged_in", method: "claude.ai" } },
    models: { kind: "unavailable", reason: "no public non-session CLI catalog" },
    integration: { kind: "known", value: "in" },
    timings: [],
  };
  return {
    machine,
    machines: [{ kind: "answered", reply: { machine, checked_at: new Date().toISOString(), elapsed_ms: 180, agents: [codex, claude] } }],
  };
}
