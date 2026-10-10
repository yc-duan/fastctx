// A world to look at the front page with: what `fastctx w`, `nodes` and
// `accounts` would answer on a desktop one morning, and what happened
// overnight. `?accounts=2` logs it in to a second hub as well, to see the
// interface grouped by account.

import type { Address } from "../world/events";
import type { HubAccounts } from "../world/accounts";
import type { About, Action, HubNodes, MachineFacts, Node, ProjectRecord, ResidentNow } from "../world/world";
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
  state: Partial<ResidentNow> & { origin?: About["origin"]; worksOn?: string },
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
      acting: machine === "desktop" ? { kind: "here" } : { kind: "accounts", accounts: [{ hub: HOME, account: "me" }] },
      ...(state.worksOn ? { works_on: state.worksOn } : {}),
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
    shell: "", path_separator: os === "windows" ? "\\" : "/", case_sensitive: os !== "windows", version: "1.0.0",
  };
}

function record(name: string, what: string, machines: string[]): ProjectRecord {
  return { version: 1, name, what, machines, places: [], canon: null, by: { kind: "user" }, on: "desktop", at: at("00:00:00") };
}

const HOME = "w-5be1fcee3751dc7a";
const ACME = "w-9a07c2d4e1b36f58";

function node(name: string, online: boolean | null, since: string, owner: string, os: string, home: string, rights: [string, boolean, boolean], through: string | null = null): Node {
  return {
    name,
    written: name,
    online,
    since: at(since),
    through,
    facts: online ? { Ok: facts(name, os, home) } : { Err: "it is offline" },
    owner,
    rights: [[rights[0], { hands: rights[1], agents: rights[2] }]],
  };
}

/** The hubs this desktop is logged in to, as `nodes` and `accounts` see
 *  them: its own, and with `two`, a second one at work. */
function hubs(two: boolean): { nodes: HubNodes[]; accounts: HubAccounts[] } {
  const link = { kind: "online" as const, since: at("01:10:02"), hub: "203.0.113.10:7443", route: { kind: "card" as const, name: "Ethernet" } };
  const nodes: HubNodes[] = [
    {
      hub: HOME,
      name: "home",
      link,
      logins: ["me"],
      home: "me",
      machines: [
        node("laptop", true, "02:04:00", "me", "windows", "C:/Users/me", ["me", true, true]),
        node("lab", true, "00:00:00", "me", "linux", "/home/me", ["me", true, true], "la"),
        node("server", true, "00:00:00", "me", "linux", "/root", ["me", true, true]),
        node("studio", false, "12:52:00", "me", "windows", "C:/Users/me", ["me", true, true]),
        node("gpu", true, "03:30:00", "ada", "linux", "/home/ada", ["me", false, true], "la"),
      ],
    },
  ];
  const member = (name: string, kind: "computer" | "phone", logins: string[], home: string | null, added: string, online: boolean, key: string, relays: { label: string; addresses: string[] }[] = []) => ({
    name, kind, logins, home, relays, added: at(added), online, since: at(added), through: null, key,
  });
  const accounts: HubAccounts[] = [
    {
      hub: HOME,
      name: "home",
      link,
      logins: ["me"],
      home: "me",
      self_registration: false,
      accounts: [
        { name: "me", admin: true, may_register: true, locked: false, opened: at("00:00:00") },
        { name: "ada", admin: false, may_register: true, locked: false, opened: at("00:00:00") },
      ],
      members: [
        member("desktop", "computer", ["me"], "me", "00:10:00", true, "5be1fcee"),
        member("laptop", "computer", ["me"], "me", "00:20:00", true, "a1c3e9d2"),
        member("lab", "computer", ["me"], "me", "00:30:00", true, "7d0e44b1", [{ label: "la", addresses: ["203.0.113.20:7444"] }]),
        member("server", "computer", ["me"], "me", "00:00:00", true, "c0ffee12"),
        member("studio", "computer", ["me"], "me", "00:40:00", false, "9e8d7c6b"),
        member("pixel", "phone", ["me"], null, "13:20:00", true, "4f2a91aa"),
        member("gpu", "computer", ["ada"], "ada", "03:30:00", true, "be3f0a77"),
      ],
      grants: [
        { machine: "gpu", account: "me", rights: { hands: false, agents: true } },
        { machine: "lab", account: "ada", rights: { hands: true, agents: false } },
      ],
      trust: [],
      projects: [
        { name: "mark50", owner: "me", granted: ["ada"] },
        ...(two ? [] : [{ name: "ingest", owner: "me", granted: [] }]),
      ],
      arrivals: [{ hub: HOME, account: "me", name: "pixel", device: true, noticed_at: at("13:20:00") }],
    },
  ];
  if (two) {
    const work = { kind: "online" as const, since: at("01:10:05"), hub: "198.51.100.7:7443", route: { kind: "card" as const, name: "Ethernet" } };
    nodes.push({
      hub: ACME,
      name: "acme",
      link: work,
      logins: ["yc"],
      home: null,
      machines: [
        node("laptop", true, "02:04:00", "yc", "windows", "C:/Users/me", ["yc", true, true]),
        node("build", true, "00:05:00", "ops", "linux", "/home/ops", ["yc", true, true]),
      ],
    });
    accounts.push({
      hub: ACME,
      name: "acme",
      link: work,
      logins: ["yc"],
      home: null,
      self_registration: false,
      accounts: [{ name: "yc", admin: false, may_register: false, locked: false, opened: at("00:00:00") }],
      members: [member("desktop", "computer", ["yc"], null, "00:12:00", true, "5be1fcee")],
      grants: [{ machine: "build", account: "yc", rights: { hands: true, agents: true } }],
      trust: [],
      projects: [{ name: "ingest", owner: "yc", granted: [] }],
      arrivals: [],
    });
  }
  return { nodes, accounts };
}

export function specimenWorld(): { world: WorldState; now: Date; lastVisit: Date } {
  const world: WorldState = {
    here: "desktop",
    // Filled from what `nodes` answers, as the shell's World is.
    nodes: [],
    machines: [
      {
        kind: "answered",
        reply: {
          machine: "laptop",
          residents: [
            resident("claude-2b1c", "laptop", claude(), "mark50", "claude-opus-5-5", "05:02:00", {
              now: { owed: {}, answering: [], activity: "busy", since: at("13:36:00"), turn: "t3", trouble: null, desk: false, waiting_for_user: false, title: "PID 环改定点" },
              last_action: action("13:37:10", "lab", "run", "cd /srv/mark50 && git fetch laptop colleague/q16-edge && cargo test -p ctrl overflow_repro", "running"),
            }),
            resident("codex-91c0", "laptop", codex(), "mark50", "gpt-6.1-sol", "05:20:00", {
              origin: { kind: "colleague", asked_by: address("laptop", "claude-2b1c"), label: "造出让 Q32 积分溢出的输入" },
              here: false,
              now: { owed: {}, answering: [], activity: "asleep", since: at("13:34:00"), turn: null, trouble: null, desk: false, waiting_for_user: false, title: null },
              last_action: action("13:33:40", "laptop", "run", "cargo test -p ctrl overflow_repro", "exit 101"),
            }),
            resident("codex-7f3a", "laptop", codex(), "ingest", "gpt-6.1-sol", "08:40:00", {
              worksOn: "lab",
              now: { owed: {}, answering: [], activity: "idle", since: at("09:31:00"), turn: null, trouble: null, desk: false, waiting_for_user: false, title: null },
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
              now: { owed: {}, answering: [], activity: "busy", since: at("13:20:00"), turn: "t2", trouble: null, desk: false, waiting_for_user: true, title: "批量提交" },
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
              now: { owed: {}, answering: [], activity: "busy", since: at("13:12:00"), turn: "t5", trouble: { at: at("13:30:00"), text: "Reconnecting... 2/5" }, desk: false, waiting_for_user: false, title: null },
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
              output: "/home/me/.fastctx/world/towns/mark50/residents/codex-3c77/out/j-p0q1r2.log",
            },
            // lab is lent to bob too: what bob runs there shows, marked as his.
            {
              job: "j-b0b7e1",
              machine: "lab",
              owner: address("bob-laptop", "codex-8e21"),
              started_at: at("11:05:00"),
              pid: 50277,
              command: "python finetune.py --gpu 0 --steps 20000",
              output: "/home/me/.fastctx/world/outside/residents/codex-8e21@bob-laptop/out/j-b0b7e1.log",
              account: { hub: "w-0123456789abcdef", account: "bob" },
            },
          ],
        },
      },
      { kind: "answered", reply: { machine: "server", residents: [], jobs: [] } },
      { kind: "offline", machine: "studio", since: at("12:52:00") },
    ],
    others: [{ machine: "lab", id: "j-b0b7e1", accounts: ["bob"] }],
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
export function specimenNow(two = false): { world: WorldNow; now: Date; lastVisit: Date; away: Away | null } {
  const { world, now, lastVisit } = specimenWorld();
  const { nodes, accounts } = hubs(two);
  // At work, the ingest service is the work account's, and so are those in it.
  if (two) {
    for (const machine of world.machines) {
      if (machine.kind !== "answered") continue;
      for (const r of machine.reply.residents) {
        if (r.town === "ingest" && r.about.machine !== "desktop") r.about.acting = { kind: "accounts", accounts: [{ hub: ACME, account: "yc" }] };
      }
    }
  }
  return {
    world: {
      here: world.here,
      who: { machine: world.here, scope: { kind: "world", towns: world.towns }, machines: world.machines, others: world.others ?? [] },
      nodes: { machine: world.here, facts: facts("desktop", "windows", "C:/Users/me"), hubs: nodes },
      towns: world.towns,
      accounts: { machine: world.here, hubs: accounts },
    },
    now,
    lastVisit,
    away: world.away,
  };
}


/** What the programs on a specimen machine say of themselves: Codex from
 *  the desktop app with its models, Claude Code logged in; on this
 *  computer, a second Codex login for work, nearly spent, and a second
 *  Claude login that was logged out; on lab, Claude Code installed but not
 *  logged in. */
export function specimenAgents(machine: string, at: Date = new Date()): WorldAgents {
  const efforts = (names: string[]) => names.map((effort) => ({ effort, description: "" }));
  const now = at.getTime();
  const ago = (minutes: number) => new Date(now - minutes * 60_000).toISOString();
  const later = (minutes: number) => new Date(now + minutes * 60_000).toISOString();
  const window = (name: string, minutes: number, used: number, resets: number) => ({ name, minutes, used_tenths: used, resets_at: later(resets) });
  const home = machine === "desktop" ? "C:/Users/me" : "/home/me";
  const codex: AgentFacts = {
    agent: "codex",
    home: { dir: `${home}/.codex`, found: "default" },
    account: { kind: "known", value: { email: "me@example.com", plan: "plus" } },
    limits: {
      kind: "known",
      value: { observed_at: ago(3), windows: [window("codex", 300, 423, 131), window("codex", 10080, 182, 3 * 1440 + 400)] },
    },
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
    unattended: { kind: "known", value: true },
    timings: [],
  };
  const work: AgentFacts = {
    ...codex,
    home: { dir: `${home}/.codex-work`, found: "found" },
    account: { kind: "known", value: { email: "me@acme.example", plan: "team" } },
    limits: {
      kind: "known",
      value: {
        observed_at: ago(1),
        windows: [window("codex", 300, 912, 47), window("codex", 10080, 1000, 2 * 1440 + 90)],
        credits: { balance: "52230.1460550000", unlimited: false },
      },
    },
    integration: { kind: "known", value: "out" },
  };
  const claude: AgentFacts = {
    agent: "claude",
    home: { dir: `${home}/.claude`, found: "default" },
    account: { kind: "known", value: machine === "lab" ? {} : { email: "me@example.com", organization: "me@example.com's Organization", plan: "max" } },
    limits:
      machine === "lab"
        ? { kind: "unavailable", reason: "no session in this home has reported its limits yet" }
        : { kind: "known", value: { observed_at: ago(26), windows: [window("five_hour", 300, 618, 188), window("seven_day", 10080, 341, 4 * 1440 + 220)] } },
    program: { kind: "known", value: { path: "C:/Users/me/.local/bin/claude.exe", source: "path", exists: true } },
    version: { kind: "known", value: "2.1.289 (Claude Code)" },
    launch: { kind: "known", value: { kind: "claude_command" } },
    login: { kind: "known", value: machine === "lab" ? { kind: "logged_out" } : { kind: "logged_in", method: "claude.ai" } },
    models: { kind: "unavailable", reason: "no public non-session CLI catalog" },
    integration: { kind: "known", value: "in" },
    unattended: { kind: "known", value: true },
    timings: [],
  };
  const personal: AgentFacts = {
    ...claude,
    home: { dir: `${home}/.claude-personal`, found: "added" },
    account: { kind: "unavailable", reason: "not logged in" },
    limits: { kind: "unavailable", reason: "no session in this home has reported its limits yet" },
    login: { kind: "known", value: { kind: "logged_out" } },
    integration: { kind: "known", value: "out" },
    unattended: { kind: "known", value: false },
  };
  const agents = machine === "desktop" ? [codex, claude, work, personal] : [codex, claude];
  return {
    machine,
    machines: [{ kind: "answered", reply: { machine, checked_at: at.toISOString(), elapsed_ms: 180, agents } }],
  };
}
