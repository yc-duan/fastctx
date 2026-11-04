// What a plain browser shows on the agents page: this desktop with two
// Codex logins listed, Claude Code listed with a long-lived token and a
// candidate under the home, two folders chosen for logins, one of them
// searched with what it found; the laptop, never scanned; the studio,
// offline. A scan runs for a few seconds and searches the folders again; a
// folder chosen is searched for a few seconds, or, named like a login,
// becomes a candidate; what a search found is taken up as a candidate.
// Times are told from `at`, the specimen's own morning.

import type { AgentLogins, ChangeCard, ChangePreview, Folder, FolderChange, FoundIn, Login, LoginChange, MachineLogins, WorldLogins } from "../world/logins";
import type { DiffLine } from "../world/run-facts";
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
const resetting = new Set<string>();
/** The logins the workbench is open in, as opening and closing it set. */
const benches = new Set<string>(["C:\\Users\\me\\.codex"]);
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
  const record = desktopAsIs(now);
  for (const side of [record.codex, record.claude]) {
    side.listed = side.listed.map((login) => ({ ...login, ...(benches.has(login.dir) ? { workbench: true } : {}), ...(resetting.has(login.dir) ? { reset_in_place: true } : {}) }));
  }
  return { ...record, by_login: true, others: otherAgents() };
}

/** The other agents a node with their adapters finds on the desktop, as it
 *  lists them: Antigravity connected, OpenCode and Cursor found. */
function otherAgents(): Record<string, AgentLogins> {
  const login = (dir: string, connected: boolean): Login => ({
    dir,
    default: true,
    found: "default",
    identity: { at: ago(52), who: { kind: "unavailable", reason: "its files name no account" } },
    limits: { kind: "unavailable", reason: "only the workbench reads limits" },
    programs: [],
    integration: connected ? "in" : "absent",
    auto_title: false,
    workbench: false,
  });
  const agent = (title: string, dir: string, connected: boolean): AgentLogins => ({
    title,
    programs: [],
    listed: connected ? [login(dir, true)] : [],
    candidates: connected ? [] : [login(dir, false)],
    elsewhere: [],
  });
  return {
    antigravity: agent("Antigravity App / CLI", "C:\\Users\\me\\.gemini", !otherOut.has("antigravity")),
    opencode: agent("OpenCode", "C:\\Users\\me\\.config\\opencode", otherIn.has("opencode")),
    cursor: agent("Cursor", "C:\\Users\\me\\.cursor", otherIn.has("cursor")),
  };
}

/** The other agents connected or taken out in the specimen. */
const otherIn = new Set<string>();
const otherOut = new Set<string>();

/** Connecting another agent, as its adapter works it out: OpenCode gets its
 *  server, instructions, shell turned off and limits; Cursor its server
 *  alone, its own shell staying on. */
function otherPreview(agent: string, dir: string, into: boolean): ChangePreview {
  const way = (lines: DiffLine[]) => (into ? lines : undone(lines));
  const card = (change: ChangeCard["change"], path: string, lines: DiffLine[]): ChangeCard => ({ change, files: [{ path, how: "changed", lines: way(lines) }] });
  const server = (at: string) =>
    told(2, 2, [
      add(`    "fastctx": {`),
      add('      "args": ['),
      add('        "serve"'),
      add("      ],"),
      add('      "command": "C:/Users/me/.fastctx/bin/fastctx.exe",'),
      add('      "env": {'),
      add('        "FASTCTX_HOME": "C:/Users/me/.fastctx"'),
      add("      }"),
      add("    },"),
    ]).map((line) => (line.kind === "hunk" ? { ...line, text: at } : line));
  const cards: ChangeCard[] =
    agent === "cursor"
      ? [card("tools", `${dir}\\mcp.json`, server(""))]
      : agent === "opencode"
        ? [
            card("tools", `${dir}\\opencode.jsonc`, server("")),
            card("shell", `${dir}\\opencode.jsonc`, told(14, 14, [add('  "permission": {'), add('    "bash": "deny"'), add("  },")])),
            card("limits", `${dir}\\opencode.jsonc`, told(14, 14, [add('  "tool_output": {'), add('    "max_bytes": 1048576,'), add('    "max_lines": 100000'), add("  },")])),
            card("guidance", `${dir}\\AGENTS.md`, told(1, 1, passage(false).map(add))),
          ]
        : [card("tools", `${dir}\\config\\mcp_config.json`, server("")), card("guidance", `${dir}\\AGENTS.md`, told(1, 1, passage(false).map(add)))];
  return { agent, dir, into, files: [], fingerprint: "3c1e9a7b5d2f4068a1c3e5f7092b4d6e", cards, ...(agent === "cursor" ? { keeps_shell: true } : {}) };
}

function desktopAsIs(now: number): MachineLogins {
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
  // alone, the workbench open in it, without its scan or its account's
  // windows.
  delete render.scan;
  render.by_login = true;
  render.claude.listed = render.claude.listed.map((login) => ({ ...login, workbench: true, limits: { kind: "unavailable", reason: "the windows of the login's account are its owner's to see" } }));
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

/** Turns counting a week cleared before its reset anew on or off in the
 *  specimen. */
export function specimenSetResetInPlace(machine: string, agent: string, dir: string, on: boolean, at: Date): Promise<MachineLogins> {
  void machine;
  void agent;
  base = at.getTime();
  if (on) resetting.add(dir);
  else resetting.delete(dir);
  return new Promise((resolve) => setTimeout(() => resolve(desktop(Date.now())), 200));
}

/** What a folder's search found, taken up as a candidate. */
export function specimenTakeUp(machine: string, agent: "codex" | "claude", dir: string, at: Date): Promise<MachineLogins> {
  void machine;
  base = at.getTime();
  if (!takenUp.some((one) => one.dir === dir)) takenUp.push({ agent, dir });
  return new Promise((resolve) => setTimeout(() => resolve(desktop(Date.now())), 900));
}

type Row = ["context" | "added" | "removed", string];
const ctx = (line: string): Row => ["context", line];
const add = (line: string): Row => ["added", line];
const del = (line: string): Row => ["removed", line];

/** A file's lines as the node tells them: `rows` from the first one shown,
 *  numbered from `before` and `after`. */
function told(before: number, after: number, rows: Row[]): DiffLine[] {
  const lines: DiffLine[] = [{ kind: "hunk", before, after, text: "", ending: "" }];
  for (const [kind, text] of rows) {
    lines.push({ kind, before: kind === "added" ? null : before, after: kind === "removed" ? null : after, text, ending: "\n" });
    if (kind !== "added") before += 1;
    if (kind !== "removed") after += 1;
  }
  return lines;
}

/** The same lines taken back out: what was added is removed, each change's
 *  removed lines before its added ones. */
function undone(lines: DiffLine[]): DiffLine[] {
  const out: DiffLine[] = [];
  let run: DiffLine[] = [];
  const settle = () => {
    out.push(...run.filter((line) => line.kind === "removed"), ...run.filter((line) => line.kind === "added"));
    run = [];
  };
  for (const line of lines) {
    const kind = line.kind === "added" ? "removed" : line.kind === "removed" ? "added" : line.kind;
    const turned: DiffLine = { ...line, kind, before: line.after, after: line.before };
    if (kind === "added" || kind === "removed") run.push(turned);
    else {
      settle();
      out.push(turned);
    }
  }
  settle();
  return out;
}

/** Opening the workbench in a login (`open`), or closing it, as the node
 *  works it out: Claude Code gets the World's plugin and the permission to
 *  work unattended, Codex the limits of the colleagues' tools. */
function benchPreview(agent: string, dir: string, open: boolean): ChangePreview {
  const way = (lines: DiffLine[]) => (open ? lines : undone(lines));
  const card = (change: ChangeCard["change"], path: string, lines: DiffLine[], extra: Partial<ChangeCard> = {}): ChangeCard => ({
    change,
    files: [{ path, how: "changed", lines: way(lines) }],
    ...extra,
  });
  const cards: ChangeCard[] =
    agent === "codex"
      ? [
          card(
            "tools",
            `${dir}\\config.toml`,
            told(15, 15, [
              ctx("[mcp_servers.fastctx.tools.run]"),
              ctx("output_token_limit = 41708"),
              add(""),
              add("[mcp_servers.fastctx.tools.ask_colleague]"),
              add("output_token_limit = 41708"),
              add(""),
              add("[mcp_servers.fastctx.tools.message_colleague]"),
              add("output_token_limit = 41708"),
            ]),
          ),
        ]
      : [
          {
            change: "plugin",
            files: [],
            commands: open
              ? ["claude plugin marketplace add C:\\Users\\me\\.fastctx\\world\\claude-plugin", "claude plugin install fastctx-world@fastctx"]
              : ["claude plugin uninstall fastctx-world@fastctx", "claude plugin marketplace remove fastctx"],
          },
          card("consent", `${dir}\\settings.json`, told(1, 1, [ctx("{"), add('  "skipDangerousModePermissionPrompt": true,'), ctx('  "permissions": {')])),
          // The plugin brings `run` with the workbench; without it, the
          // entry in Claude Code's state does.
          {
            change: "tools",
            files: [
              {
                path: "C:\\Users\\me\\.claude.json",
                how: "changed",
                lines: (open ? undone : (lines: DiffLine[]) => lines)(
                  told(2, 2, [
                    add('  "mcpServers": {'),
                    add('    "fastctx": {'),
                    add('      "args": ['),
                    add('        "serve"'),
                    add("      ],"),
                    add('      "command": "C:/Users/me/.fastctx/bin/fastctx.exe",'),
                    add('      "env": {'),
                    add('        "FASTCTX_AGENT_HOME": "",'),
                    add('        "FASTCTX_HOME": "C:/Users/me/.fastctx"'),
                    add("      },"),
                    add('      "type": "stdio"'),
                    add("    }"),
                    add("  },"),
                  ]),
                ),
              },
            ],
          },
        ];
  return { agent, dir, into: true, files: [], fingerprint: "7a1d3c5e9b2f4a6c8e0d1b3f5a7c9e2d", cards };
}

/** The World's passage in an agent's instructions, as `briefing.rs` writes it. */
const passage = (codex: boolean): string[] => [
  "<!-- fastctx:begin -->",
  "## Your shell and tools",
  "",
  "You work through fastctx's `run` tool. `run` is your shell: bash with the GNU tools and `rg`, Windows included. Do all shell work with it, on this machine too. On every machine it reaches, it also works on files:",
  "- `fastctx view PATH...` shows files: text with line numbers (`--lines 120-180` for part of one); an image as the image itself, for you to see; a PDF as its text by page (`--pages 3-5`, and `--image` to see the pages); Word, PowerPoint and Excel as their content.",
  "- `edit PATH` and `write PATH` change a file and keep its encoding and line endings.",
  "- `rg` searches.",
  "",
  "`run`'s description shows each of these in full.",
  "",
  "`run` runs each script on one of the machines its description lists, this computer among them, the whole script written as one `on MACHINE`; its description says how. When the user speaks of one of those machines, work on it.",
  "",
  "Inside a script, these commands are ordinary programs: give them several files, pipe into and out of them, loop over machines, as with any command. Calls to `run` share nothing: make several at once, or call `run` from your scripts and subagents, as with any tool.",
  ...(codex
    ? [
        "",
        "## fastctx in Codex",
        "",
        "You must begin every `exec` script with this exact first line, every time:",
        "",
        '    // @exec: {"max_output_tokens": 60000}',
        "",
        "fastctx answers are long, and without that line you see only their first part. Put any other setting, such as `yield_time_ms`, into that same line.",
      ]
    : []),
  "<!-- fastctx:end -->",
];

/** What connecting the login does, a card each kind of change, as the node
 *  works it out: Codex keeps its server, its limit and its shell in
 *  `config.toml`; Claude Code its server in `.claude.json` beside its
 *  folder, and its limits and shell in `settings.json`. */
function disclosed(agent: string, dir: string, into: boolean): Pick<ChangePreview, "cards" | "fingerprint" | "output_limit"> {
  const way = (lines: DiffLine[]) => (into ? lines : undone(lines));
  const how = into ? "created" : "removed";
  const card = (change: ChangeCard["change"], path: string, lines: DiffLine[], made: "created" | "changed" = "changed"): ChangeCard => ({
    change,
    files: [{ path, how: made === "created" ? how : "changed", lines: way(lines) }],
  });
  const fingerprint = "5f0c2a9e41d7b3c86e1f0a2b9c4d7e13";
  if (agent === "codex") {
    const config = `${dir}\\config.toml`;
    return {
      fingerprint,
      ...(into ? { output_limit: { location: `${config}: tool_output_token_limit`, current: 10000, requested: 41708 } } : {}),
      cards: [
        card("limits", config, told(1, 1, [ctx('model = "gpt-5.5"'), ctx('model_reasoning_effort = "high"'), del("tool_output_token_limit = 10000"), add("tool_output_token_limit = 41708"), ctx(""), ctx("[mcp_servers.github]")])),
        card(
          "tools",
          config,
          told(6, 6, [
            ctx('command = "github-mcp-server"'),
            ctx('env = { GITHUB_TOKEN = "•••" }'),
            add(""),
            add("[mcp_servers.fastctx]"),
            add("command = 'C:\\Users\\me\\.fastctx\\bin\\fastctx.exe'"),
            add('args = ["serve"]'),
            add('env_vars = ["CODEX_HOME"]'),
            add("startup_timeout_sec = 120"),
            add("tool_timeout_sec = 660"),
            add('omit_tools_from = ["deferred"]'),
            add(""),
            add("[mcp_servers.fastctx.tools.run]"),
            add("output_token_limit = 41708"),
          ]),
        ),
        card("guidance", `${dir}\\AGENTS.md`, told(1, 1, passage(true).map(add)), "created"),
        card("shell", config, told(17, 17, [ctx("[mcp_servers.fastctx.tools.run]"), ctx("output_token_limit = 41708"), add(""), add("[features]"), add("shell_tool = false")])),
      ],
    };
  }
  const home = /\\\.claude$/i.test(dir) ? dir.replace(/\\\.claude$/i, "") : dir;
  const settings = `${dir}\\settings.json`;
  return {
    fingerprint,
    cards: [
      // Fastctx's entry alone: the rest of the state is the user's account
      // and history, and the node tells none of it.
      card(
        "tools",
        `${home}\\.claude.json`,
        told(2, 2, [
          add('  "mcpServers": {'),
          add('    "fastctx": {'),
          add('      "args": ['),
          add('        "serve"'),
          add("      ],"),
          add('      "command": "C:/Users/me/.fastctx/bin/fastctx.exe",'),
          add('      "env": {'),
          add('        "FASTCTX_AGENT_HOME": "",'),
          add('        "FASTCTX_HOME": "C:/Users/me/.fastctx"'),
          add("      },"),
          add('      "type": "stdio"'),
          add("    }"),
          add("  },"),
        ]),
      ),
      card(
        "limits",
        settings,
        told(4, 4, [
          ctx('    "type": "command",'),
          ctx('    "command": "~/.claude/statusline.sh"'),
          del("  }"),
          add("  },"),
          add('  "env": {'),
          add('    "MAX_MCP_OUTPUT_TOKENS": "100000",'),
          add('    "CLAUDE_CODE_MAX_MCP_DESCRIPTION_LENGTH": "16384"'),
          add("  }"),
          ctx("}"),
        ]),
      ),
      card(
        "shell",
        settings,
        told(8, 8, [
          ctx('    "MAX_MCP_OUTPUT_TOKENS": "100000",'),
          ctx('    "CLAUDE_CODE_MAX_MCP_DESCRIPTION_LENGTH": "16384"'),
          del("  }"),
          add("  },"),
          add('  "permissions": {'),
          add('    "deny": ['),
          add('      "Bash",'),
          add('      "PowerShell"'),
          add("    ]"),
          add("  }"),
          ctx("}"),
        ]),
      ),
      card("guidance", `${dir}\\CLAUDE.md`, told(1, 1, [ctx("# 我的习惯"), ctx(""), ctx("- 回答用中文。"), add(""), ...passage(false).map(add)])),
    ],
  };
}

export function specimenPreview(machine: string, agent: string, dir: string, into: boolean, workbench?: boolean): Promise<ChangePreview> {
  void machine;
  if (workbench !== undefined) return Promise.resolve(benchPreview(agent, dir, workbench));
  if (agent !== "codex" && agent !== "claude") return Promise.resolve(otherPreview(agent, dir, into));
  // As the node gives them: the World's own values, any other key named
  // with its values withheld, and the World's section of the guidance file.
  const way = (value: string) => (into ? { after: value } : { before: value });
  const how = into ? "created" : "removed";
  const section = ["<!-- fastctx:begin -->", "## Fastctx", "", "Run commands, read and edit files and reach the other machines through `run`.", "A long command goes on in the background; you are told when it ends.", "<!-- fastctx:end -->", ""].join("\n");
  const made: ChangePreview =
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
                { item: "mcp_servers.fastctx.args", how, ...way(JSON.stringify(["serve"])) },
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
                { item: "env.MAX_MCP_OUTPUT_TOKENS", how, ...way(JSON.stringify("100000")) },
                { item: "permissions.deny", how, ...way(JSON.stringify(["Bash", "PowerShell"])) },
              ],
            },
            { path: `${dir}\\CLAUDE.md`, how: "changed", items: [{ item: "fastctx", how, ...way(section) }] },
          ],
        };
  return Promise.resolve({ ...made, ...disclosed(agent, dir, into) });
}

export function specimenChange(machine: string, agent: string, dir: string, into: boolean, workbench?: boolean): Promise<LoginChange> {
  void machine;
  if (agent !== "codex" && agent !== "claude") {
    (into ? otherIn : otherOut).add(agent);
    (into ? otherOut : otherIn).delete(agent);
  }
  if (workbench === true && into) benches.add(dir);
  else if (workbench === false || !into) benches.delete(dir);
  return new Promise((done) =>
    setTimeout(
      () => done({ into, started_at: ago(0), steps: [{ step: { step: "program" }, state: { state: "done" } }, { step: { step: "files" }, state: { state: "done" } }, { step: { step: "check", scope: "user" }, state: { state: "done" } }], end: { end: "done", at: ago(0) } }),
      1600,
    ),
  );
}
