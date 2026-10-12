// What a plain browser shows on the agents page: this desktop with two
// Codex logins listed and a third found by the walk, Claude Code listed with
// a long-lived token and a candidate under the home, and the laptop, which
// was never scanned. A scan runs for a few seconds and finds what it found.

import type { ChangePreview, LoginChange, MachineLogins, WorldLogins } from "../world/logins";

const ago = (minutes: number) => new Date(Date.now() - minutes * 60_000).toISOString();
const later = (minutes: number) => new Date(Date.now() + minutes * 60_000).toISOString();

const limits = (five: number, week: number) => ({
  kind: "known" as const,
  value: {
    observed_at: ago(3),
    windows: [
      { name: "five_hour", minutes: 300, used_tenths: five, resets_at: later(140) },
      { name: "seven_day", minutes: 10080, used_tenths: week, resets_at: later(3 * 1440) },
    ],
  },
});

function desktop(scanning: boolean): MachineLogins {
  return {
    machine: "desktop",
    scan: scanning
      ? { started_at: ago(0), usual: { state: "done", at: ago(0) }, disks: { stage: { state: "running" }, disks: [{ root: "C:\\", directories: 184_220, unreadable: 0, done: false }, { root: "E:\\", directories: 61_034, unreadable: 0, done: true }] } }
      : { started_at: ago(52), usual: { state: "done", at: ago(52) }, disks: { stage: { state: "done", at: ago(49) }, disks: [{ root: "C:\\", directories: 545_716, unreadable: 0, done: true }, { root: "E:\\", directories: 195_244, unreadable: 0, done: true }] } },
    codex: {
      programs: [
        { path: "C:\\Users\\me\\AppData\\Roaming\\npm\\codex.cmd", how: "npm", version: "0.160.0", on_path: true },
        { how: "desktop_app", within: { name: "ChatGPT", id: "OpenAI.ChatGPT", version: "26.1007.2314.0", path: "C:\\Program Files\\WindowsApps\\OpenAI.ChatGPT" }, on_path: false },
      ],
      listed: [
        { dir: "C:\\Users\\me\\.codex", default: true, found: "default", identity: { at: ago(52), who: { kind: "known", value: { logged_in: true, method: "chatgpt", email: "me@example.com", plan: "pro" } } }, limits: limits(412, 233), programs: [0, 1], integration: "in" },
        { dir: "C:\\Users\\me\\.codex-work", default: false, found: "home", identity: { at: ago(52), who: { kind: "known", value: { logged_in: true, method: "chatgpt", email: "work@example.com", plan: "plus" } } }, limits: limits(880, 610), programs: [0], integration: "in" },
      ],
      candidates: [],
      elsewhere: [{ dir: "D:\\backup\\.codex-old", traces: ["auth.json", "config.toml"] }],
    },
    claude: {
      programs: [{ path: "C:\\Users\\me\\.local\\bin\\claude.exe", how: "official", version: "2.1.296", on_path: true }],
      listed: [
        { dir: "C:\\Users\\me\\.claude", default: true, found: "default", identity: { at: ago(52), who: { kind: "known", value: { logged_in: true, method: "oauth_token" } } }, limits: { kind: "unavailable", reason: "not read yet" }, programs: [0], integration: "in", unattended: true },
      ],
      candidates: [
        { dir: "C:\\Users\\me\\.claude-max", default: false, found: "home", identity: { at: ago(52), who: { kind: "known", value: { logged_in: true, method: "claude.ai", email: "me@example.com", plan: "max" } } }, limits: limits(120, 300), programs: [0], integration: "out" },
      ],
      elsewhere: [],
    },
  };
}

let scanUntil = 0;

export function specimenLogins(machine: string | null): Promise<WorldLogins> {
  const scanning = Date.now() < scanUntil;
  const all: WorldLogins = {
    machine: "desktop",
    machines: [
      { kind: "answered", logins: desktop(scanning) },
      { kind: "answered", logins: { machine: "laptop", codex: { programs: [], listed: [], candidates: [], elsewhere: [] }, claude: { programs: [], listed: [], candidates: [], elsewhere: [] } } },
      { kind: "offline", machine: "studio", since: ago(12 * 60) },
    ],
  };
  return Promise.resolve(machine ? { ...all, machines: all.machines.filter((one) => (one.kind === "answered" ? one.logins.machine : one.machine) === machine) } : all);
}

export function specimenScan(machine: string, stop: boolean): Promise<MachineLogins> {
  scanUntil = stop ? 0 : Date.now() + 6000;
  return Promise.resolve(desktop(!stop && machine === "desktop"));
}

export function specimenPreview(machine: string, agent: "codex" | "claude", dir: string, into: boolean): Promise<ChangePreview> {
  void machine;
  return Promise.resolve(
    agent === "codex"
      ? { agent, dir, into, files: [{ path: `${dir}\\config.toml`, how: "changed", items: [{ item: "mcp_servers.fastctx", how: into ? "created" : "removed" }, { item: "features.shell_tool", how: "changed", before: "true", after: "false" }] }, { path: `${dir}\\AGENTS.md`, how: "changed", items: [{ item: "fastctx guidance", how: into ? "created" : "removed" }] }] }
      : { agent, dir, into, files: [{ path: `${dir}\\CLAUDE.md`, how: "changed", items: [{ item: "fastctx guidance", how: into ? "created" : "removed" }] }], commands: [`claude plugin ${into ? "install" : "uninstall"} fastctx-world@fastctx`] },
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
