// What a plain browser shows on the agents page: this desktop with two
// Codex logins listed, Claude Code listed with a long-lived token and a
// candidate under the home, two folders chosen for logins, one of them
// searched with what it found; the laptop, never scanned; the studio,
// offline. A scan runs for a few seconds and searches the folders again; a
// folder chosen is searched for a few seconds, or, named like a login,
// becomes a candidate; what a search found is taken up as a candidate.
// Times are told from `at`, the specimen's own morning.

import type { ChangePreview, Folder, FolderChange, FoundIn, Login, LoginChange, MachineLogins, WorldLogins } from "../world/logins";
import { specimenLimits } from "./usage";

let base = Date.now();
const ago = (minutes: number) => new Date(base - minutes * 60_000).toISOString();
const later = (minutes: number) => new Date(base + minutes * 60_000).toISOString();

/** A login's windows as last read: the five-hour one resets `fiveResets`
 *  minutes from now, past already for a reading older than its reset, and
 *  a plan with a weekly window alone has no five-hour one. */
const limits = (five: number, week: number, shape: { weekResets?: number; fiveResets?: number; weekOnly?: boolean } = {}) => {
  const { weekResets = 3 * 1440, fiveResets = 140, weekOnly = false } = shape;
  return {
    kind: "known" as const,
    value: {
      observed_at: ago(fiveResets < 0 ? 52 : 3),
      windows: [
        ...(weekOnly ? [] : [{ name: "five_hour", minutes: 300, used_tenths: five, resets_at: later(fiveResets) }]),
        { name: "seven_day", minutes: 10080, used_tenths: week, resets_at: later(weekResets) },
      ],
    },
  };
};

/** The limits a listed login says now: those its usage specimen climbs
 *  to, so its rings and its readings agree. */
const limitsOf = (machine: string, agent: string, dir: string, five: number, week: number, fiveResets = 140) => {
  const said = specimenLimits(machine, agent, dir);
  return said ? limits(said.five, said.week, { weekResets: said.resetsIn / 60_000, fiveResets, weekOnly: said.weekOnly }) : limits(five, week, { fiveResets });
};

const HOMES = "C:\\Users\\me\\CodexHomes";

/** What the desktop's folders hold, found as a search reaches them. */
const below: Record<string, FoundIn[]> = {
  [HOMES]: [
    { agent: "codex", path: "team", traces: ["auth.json", "config.toml", "sessions"] },
    { agent: "codex", path: "trial", traces: ["auth.json", "history.jsonl"] },
    { agent: "codex", path: "2024\\old-plus", traces: ["config.toml", "sessions", "state_5.sqlite"] },
    { agent: "claude", path: "claude-review", traces: [".credentials.json", "projects"] },
  ],
  "D:\\": [{ agent: "codex", path: "backup\\.codex-old", traces: ["auth.json", "config.toml"] }],
};

/** A folder chosen, with when its search began and, once it ended, how. */
interface Chosen {
  dir: string;
  began: number;
  ended: { at: number; stopped: boolean } | null;
}

/** How long a search takes here, and how many folders it sees a second. */
const SEARCH_MS = 5200;
const PER_SECOND = 9_400;

let chosen: Chosen[] = [
  { dir: HOMES, began: Date.now() - 50 * 60_000, ended: { at: Date.now() - 50 * 60_000 + SEARCH_MS, stopped: false } },
  { dir: "D:\\", began: Date.now() - 49 * 60_000, ended: { at: Date.now() - 48 * 60_000, stopped: true } },
];
/** Candidates taken up from what a search found. */
let takenUp: { agent: "codex" | "claude"; dir: string }[] = [];
let scanUntil = 0;
/** Codex logins the specimen does not name conversations in, as the
 *  toggle sets: naming is on unless turned off. */
const untitled = new Set<string>();
/** When the laptop was scanned, finding nothing; never, at first. */
let laptopScanned: number | null = null;

function search(one: Chosen, now: number): Folder {
  const running = !one.ended && now - one.began < SEARCH_MS;
  if (!running && !one.ended) one.ended = { at: one.began + SEARCH_MS, stopped: false };
  const until = one.ended ? one.ended.at : now;
  const elapsed = Math.max(0, until - one.began);
  const share = Math.min(1, elapsed / SEARCH_MS);
  const all = below[one.dir] ?? [];
  const found = all.slice(0, Math.floor(all.length * share + 0.001)).filter((it) => !takenUp.some((taken) => taken.dir === `${one.dir.replace(/\\$/, "")}\\${it.path}`));
  const at = new Date(until).toISOString();
  return {
    dir: one.dir,
    search: {
      stage: running ? { state: "running" } : one.ended?.stopped ? { state: "stopped", at } : { state: "done", at },
      directories: Math.round((elapsed / 1000) * PER_SECOND),
      unreadable: one.dir === "D:\\" ? 3 : 0,
      found,
    },
  };
}

function candidate(agent: "codex" | "claude", dir: string): Login {
  const name = dir.split("\\").pop() ?? dir;
  return { dir, default: false, found: "elsewhere", identity: { at: ago(0), who: { kind: "known", value: { logged_in: true, method: agent === "codex" ? "chatgpt" : "claude.ai", email: `${name}@example.com`, plan: "plus" } } }, limits: limits(60, 140), programs: [0], integration: "out" };
}

// `?drift`: the World's tools in the desktop's two Codex logins wait for
// the user, one changed since, one for its output limit.
const drifting = new URLSearchParams(location.search).has("drift");

function desktop(now: number): MachineLogins {
  const scanning = now < scanUntil - 4000;
  return {
    machine: "desktop",
    scan: scanning ? { started_at: ago(0), usual: { state: "running" } } : { started_at: ago(52), usual: { state: "done", at: ago(52) } },
    codex: {
      programs: [
        { path: "C:\\Users\\me\\AppData\\Roaming\\npm\\codex.cmd", how: "npm", version: "0.160.0", on_path: true },
        { how: "desktop_app", within: { name: "ChatGPT", id: "OpenAI.ChatGPT", version: "26.1007.2314.0", path: "C:\\Program Files\\WindowsApps\\OpenAI.ChatGPT" }, on_path: false },
      ],
      listed: [
        { dir: "C:\\Users\\me\\.codex", default: true, found: "default", identity: { at: ago(52), who: { kind: "known", value: { logged_in: true, method: "chatgpt", email: "me@example.com", plan: "pro" } } }, limits: limitsOf("desktop", "codex", "C:\\Users\\me\\.codex", 412, 233), programs: [0, 1], integration: "in", auto_title: !untitled.has("C:\\Users\\me\\.codex"), ...(drifting ? { drift: { kind: "changed" as const, location: "C:\\Users\\me\\.codex\\AGENTS.md", reason: "the fastctx block differs from the recorded bytes" } } : {}) },
        { dir: "C:\\Users\\me\\.codex-work", default: false, found: "home", identity: { at: ago(52), who: { kind: "known", value: { logged_in: true, method: "chatgpt", email: "work@example.com", plan: "plus" } } }, limits: limitsOf("desktop", "codex", "C:\\Users\\me\\.codex-work", 880, 610), programs: [0], integration: "in", auto_title: !untitled.has("C:\\Users\\me\\.codex-work"), ...(drifting ? { drift: { kind: "asks" as const, limit: { location: "C:\\Users\\me\\.codex-work\\config.toml", current: 10000, requested: 30000 } } } : {}) },
      ],
      candidates: takenUp.filter((one) => one.agent === "codex").map((one) => candidate("codex", one.dir)),
      elsewhere: [],
    },
    claude: {
      programs: [{ path: "C:\\Users\\me\\.local\\bin\\claude.exe", how: "official", version: "2.1.303", on_path: true }],
      listed: [
        { dir: "C:\\Users\\me\\.claude", default: true, found: "default", identity: { at: ago(52), who: { kind: "known", value: { logged_in: true, method: "oauth_token" } } }, limits: limitsOf("desktop", "claude", "C:\\Users\\me\\.claude", 410, 640, -25), programs: [0], integration: "in", unattended: true },
      ],
      candidates: [
        { dir: "C:\\Users\\me\\.claude-max", default: false, found: "home", identity: { at: ago(52), who: { kind: "known", value: { logged_in: true, method: "claude.ai", email: "me@example.com", plan: "max" } } }, limits: limits(120, 300), programs: [0], integration: "out" },
        ...takenUp.filter((one) => one.agent === "claude").map((one) => candidate("claude", one.dir)),
      ],
      elsewhere: [],
    },
    folders: chosen.map((one) => search(one, now)),
  };
}

export function specimenLogins(machine: string | null, at: Date): Promise<WorldLogins> {
  base = at.getTime();
  const all: WorldLogins = {
    machine: "desktop",
    machines: [
      { kind: "answered", logins: desktop(Date.now()) },
      { kind: "answered", logins: laptop() },
      { kind: "offline", machine: "studio", since: ago(12 * 60) },
      ...others(),
    ],
  };
  if (!machine) return Promise.resolve(all);
  const found = all.machines.filter((one) => (one.kind === "answered" ? one.logins.machine : one.machine) === machine);
  return Promise.resolve({ ...all, machines: found.length ? found : [{ kind: "no_answer", machine, reason: "the specimen has no record of it" }] });
}

/** A machine scanned with one listed login of `agent`: its default one,
 *  and the program that uses it where `programs`. */
function scanned(machine: string, agent: "codex" | "claude", dir: string, who: string | null, plan: string | null, programs: boolean): MachineLogins {
  const empty = { programs: [], listed: [], candidates: [], elsewhere: [] };
  // bo's gpu keeps a Claude Code older than the World starts conversations in.
  const program = { path: `/usr/local/bin/${agent}`, how: "npm" as const, version: agent === "codex" ? "0.160.0" : machine === "gpu" ? "2.1.296" : "2.1.303", on_path: true };
  const login: Login = {
    dir,
    default: true,
    found: "default",
    identity: { at: ago(30), who: { kind: "known", value: { logged_in: true, method: agent === "codex" ? "chatgpt" : "claude.ai", ...(who ? { email: who } : {}), ...(plan ? { plan } : {}) } } },
    limits: specimenLimits(machine, agent, dir) ? limitsOf(machine, agent, dir, 0, 0) : { kind: "unavailable", reason: "not read yet" },
    programs: programs ? [0] : [],
    integration: "in",
  };
  return { machine, scan: { started_at: ago(300), usual: { state: "done", at: ago(300) } }, codex: empty, claude: empty, [agent]: { ...empty, programs: programs ? [program] : [], listed: [login] } };
}

/** The other machines the specimen's user may look at: lab and server,
 *  their own; bo's gpu, which they see as the administrator; at work, the
 *  build shared with them and render, whose Claude Code assists them. */
function others(): WorldLogins["machines"] {
  const lab = scanned("lab", "codex", "/home/me/.codex", "me@example.com", "pro", true);
  const server = scanned("server", "codex", "/root/.codex", null, null, false);
  const render = scanned("render", "claude", "/home/ops/.claude", "ops@acme.example", "max", false);
  // What render shows an account it assists: its listed assisting login
  // alone, without its scan or its account's windows.
  delete render.scan;
  render.claude.listed = render.claude.listed.map((login) => ({ ...login, limits: { kind: "unavailable", reason: "the windows of the login's account are its owner's to see" } }));
  return [
    { kind: "answered", logins: { ...lab, codex: { ...lab.codex, listed: lab.codex.listed.map((login) => ({ ...login, limits: { kind: "unavailable", reason: "not read yet" } })) } } },
    { kind: "answered", logins: { ...server, codex: { programs: [], listed: [], candidates: [], elsewhere: [] } } },
    { kind: "answered", logins: scanned("gpu", "claude", "/home/bo/.claude", "bo@example.com", "max", true) },
    { kind: "answered", logins: scanned("build", "codex", "/home/ops/.codex", "ops@acme.example", "team", true) },
    { kind: "answered", logins: render },
  ];
}

function laptop(): MachineLogins {
  const empty = { programs: [], listed: [], candidates: [], elsewhere: [] };
  const scan = laptopScanned === null ? undefined : Date.now() - laptopScanned < 1400 ? { started_at: ago(0), usual: { state: "running" as const } } : { started_at: ago(0), usual: { state: "done" as const, at: ago(0) } };
  return { machine: "laptop", ...(scan ? { scan } : {}), codex: empty, claude: empty };
}

export function specimenScan(machine: string, stop: boolean, at: Date): Promise<MachineLogins> {
  base = at.getTime();
  const now = Date.now();
  if (machine === "laptop") {
    laptopScanned = now;
    return Promise.resolve(laptop());
  }
  if (stop) {
    for (const one of chosen) if (!one.ended) one.ended = { at: now, stopped: true };
  } else {
    scanUntil = now + 5200;
    chosen = chosen.map((one) => ({ dir: one.dir, began: now + 1200, ended: null }));
  }
  return Promise.resolve(desktop(now));
}

/** A folder chosen, stopped or forgotten on the desktop. */
export function specimenFolder(machine: string, dir: string, change: FolderChange, at: Date): Promise<MachineLogins> {
  void machine;
  base = at.getTime();
  const now = Date.now();
  if (change.change === "choose") {
    const name = dir.replace(/[\\/]+$/, "").split(/[\\/]/).pop() ?? "";
    const agent = name.startsWith(".claude") ? "claude" : name.startsWith(".codex") ? "codex" : null;
    if (agent) {
      if (!takenUp.some((one) => one.dir === dir)) takenUp.push({ agent, dir });
    } else {
      chosen = [...chosen.filter((one) => one.dir !== dir), { dir, began: now, ended: null }];
      below[dir] ??= [{ agent: "codex", path: "profiles\\lab", traces: ["auth.json", "sessions"] }];
    }
  } else if (change.change === "stop") {
    for (const one of chosen) if (one.dir === dir && !one.ended) one.ended = { at: now, stopped: true };
  } else {
    chosen = chosen.filter((one) => one.dir !== dir);
  }
  return new Promise((resolve) => setTimeout(() => resolve(desktop(Date.now())), 240));
}

/** Turns naming conversations in a Codex login on or off in the specimen. */
export function specimenSetTitle(machine: string, agent: string, dir: string, on: boolean, at: Date): Promise<MachineLogins> {
  void machine;
  void agent;
  base = at.getTime();
  if (on) untitled.delete(dir);
  else untitled.add(dir);
  return new Promise((resolve) => setTimeout(() => resolve(desktop(Date.now())), 200));
}

/** What a folder's search found, taken up as a candidate. */
export function specimenTakeUp(machine: string, agent: "codex" | "claude", dir: string, at: Date): Promise<MachineLogins> {
  void machine;
  base = at.getTime();
  if (!takenUp.some((one) => one.dir === dir)) takenUp.push({ agent, dir });
  return new Promise((resolve) => setTimeout(() => resolve(desktop(Date.now())), 900));
}

export function specimenPreview(machine: string, agent: "codex" | "claude", dir: string, into: boolean): Promise<ChangePreview> {
  void machine;
  // As the node gives them: the World's own values, any other key named
  // with its values withheld, and the World's section of the guidance file.
  const way = (value: string) => (into ? { after: value } : { before: value });
  const how = into ? "created" : "removed";
  const section = ["<!-- fastctx:begin -->", "## Fastctx", "", "Run commands, read and edit files and reach the other machines through `run`.", "A long command goes on in the background; you are told when it ends.", "<!-- fastctx:end -->", ""].join("\n");
  return Promise.resolve(
    agent === "codex"
      ? {
          agent,
          dir,
          into,
          files: [
            {
              path: `\\\\?\\${dir}\\config.toml`,
              how: "changed",
              items: [
                { item: "tool_output_token_limit", how: "changed", before: into ? "10000" : "41708", after: into ? "41708" : "10000" },
                { item: "features.shell_tool", how, ...way("false") },
                { item: "mcp_servers.fastctx.command", how, ...way(JSON.stringify("C:\\Users\\me\\.fastctx\\bin\\fastctx.exe")) },
                { item: "mcp_servers.fastctx.args", how, ...way(JSON.stringify(["mcp"])) },
                { item: "mcp_servers.fastctx.tool_timeout_sec", how, ...way("660") },
                { item: "mcp_servers.fastctx.tools.run.output_token_limit", how, ...way("41708") },
                { item: "mcp_servers.github.env.GITHUB_TOKEN", how: "changed", withheld: true },
              ],
            },
            { path: `${dir}\\AGENTS.md`, how: "changed", items: [{ item: "fastctx", how, ...way(section) }] },
          ],
        }
      : {
          agent,
          dir,
          into,
          files: [
            {
              path: `${dir}\\settings.json`,
              how: "changed",
              items: [
                { item: "env.MAX_MCP_OUTPUT_TOKENS", how, ...way(JSON.stringify("30000")) },
                { item: "permissions.deny", how, ...way(JSON.stringify(["Bash", "PowerShell"])) },
                { item: 'enabledPlugins."fastctx-world@fastctx"', how, ...way("true") },
              ],
            },
            { path: `${dir}\\CLAUDE.md`, how: "changed", items: [{ item: "fastctx", how, ...way(section) }] },
          ],
          commands: [`claude plugin ${into ? "install" : "uninstall"} fastctx-world@fastctx`],
        },
  );
}

export function specimenChange(machine: string, agent: "codex" | "claude", dir: string, into: boolean): Promise<LoginChange> {
  void machine;
  void agent;
  void dir;
  return new Promise((done) =>
    setTimeout(
      () => done({ into, started_at: ago(0), steps: [{ step: { step: "program" }, state: { state: "done" } }, { step: { step: "files" }, state: { state: "done" } }, { step: { step: "check", scope: "user" }, state: { state: "done" } }], end: { end: "done", at: ago(0) } }),
      1600,
    ),
  );
}
