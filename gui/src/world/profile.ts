// An account's own layer of configuration (`world-user-config.md`), as
// `fastctx_types::profile` and `fastctx_types::mcp` write it: the prompt
// every agent working for the account is given, the settings it may set for
// its own sessions, and its MCP servers. The hub keeps the prompt, the
// settings and the list of servers; a server a command starts is named there
// only by the machine that keeps its command.

import { invoke } from "@tauri-apps/api/core";

export interface RemoteMcp {
  url: string;
  /** The older SSE transport; the streamable HTTP one otherwise. */
  sse?: boolean;
  headers?: Record<string, string>;
}

export interface CommandMcp {
  command: string;
  args?: string[];
  env?: Record<string, string>;
  /** An absolute path as its machine writes it; the user's home there when
   *  none. */
  cwd?: string;
}

/** One server in the account's list. */
export type Entry = ({ kind: "remote" } & RemoteMcp) | { kind: "command"; machine: string } | { kind: "unknown" };

export interface Profile {
  version: number;
  /** Whether project memory is on (`world-memory.md`); absent, it is. Its
   *  own commands read and change it. */
  memory?: boolean;
  prompt?: string;
  /** Claude Code settings, of `CLAUDE_SETTINGS` alone. */
  claude?: Record<string, unknown>;
  /** Codex settings, of `CODEX_SETTINGS` alone. */
  codex?: Record<string, unknown>;
  servers?: Record<string, Entry>;
}

/** The account's profile as its device reads it to change it. */
export interface ProfileView {
  hub: string;
  account: string;
  profile: Profile;
  /** Whether the hub keeps profiles at all: an older one does not. */
  kept: boolean;
}

/** One server as an `mcpServers` entry describes it. */
export type Server = ({ kind: "command" } & CommandMcp) | ({ kind: "remote" } & RemoteMcp);

export type McpCall =
  | { do: "list" }
  /** Adds or replaces it: a remote one on the account, a command one on
   *  `machine`, or where none is named, where the node chooses. */
  | { do: "keep"; name: string; server: Server; machine?: string }
  | { do: "remove"; name: string }
  | { do: "test"; name: string }
  /** Takes what one of them has on `machine` as `as_name`: one a command
   *  starts is kept on that machine, its command never leaving it; one at
   *  an address is read there again and kept on the account. */
  | { do: "import"; machine: string; source: Source; name: string; as_name: string }
  /** What Claude Desktop, Claude Code and Cursor on `machine` have. */
  | { do: "found"; machine?: string }
  /** What they have on `machine`, by name and kind alone: no address,
   *  header or command leaves it before one is imported. */
  | { do: "candidates"; machine?: string };

/** What the machine of a command server says of it; what it keeps never
 *  leaves it. */
export type CommandState =
  | { state: "kept" }
  | { state: "offline" }
  | { state: "missing" }
  | { state: "older" }
  | { state: "failed"; problem: string };

export type ListedServer = ({ kind: "remote" } & RemoteMcp) | { kind: "command"; machine: string; state: CommandState } | { kind: "unknown" };

export interface Listed {
  name: string;
  server: ListedServer;
}

export interface Tested {
  tools: string[];
  problem?: string;
  /** The end of what it wrote to its standard error. */
  log?: string;
}

export type Source = { app: "claude_desktop" } | { app: "claude_code"; home: string } | { app: "cursor" };

/** What a tool on a machine has configured: of one a command starts, only
 *  that it is one. */
export type FoundServer = { kind: "command" } | ({ kind: "remote" } & RemoteMcp);

export interface Found {
  source: Source;
  name: string;
  server: FoundServer;
}

/** One server a tool on a machine has, as importing lists it. */
export interface ImportCandidate {
  source: Source;
  name: string;
  kind: "command" | "remote";
}

export type McpReply =
  | { reply: "listed"; servers: Listed[] }
  | { reply: "kept"; name: string; version: number }
  | { reply: "removed"; version: number }
  | ({ reply: "tested" } & Tested)
  | { reply: "found"; machine: string; servers: Found[] }
  | { reply: "candidates"; machine: string; servers: ImportCandidate[] };

/** The Claude Code settings an account may set for itself
 *  (`fastctx_types::profile::CLAUDE_SETTINGS`). */
export const CLAUDE_SETTINGS = ["attribution", "autoCompactEnabled", "autoCompactWindow", "effortLevel", "env", "includeGitInstructions", "language", "model", "outputStyle", "showThinkingSummaries"] as const;

/** The variables it may set in their `env` (`fastctx_types::profile::CLAUDE_ENV`). */
export const CLAUDE_ENV = ["CLAUDE_AUTOCOMPACT_PCT_OVERRIDE"] as const;

/** The Codex settings it may (`fastctx_types::profile::CODEX_SETTINGS`). */
export const CODEX_SETTINGS = [
  "hide_agent_reasoning",
  "model",
  "model_auto_compact_token_limit",
  "model_context_window",
  "model_reasoning_effort",
  "model_reasoning_summary",
  "model_verbosity",
  "show_raw_agent_reasoning",
] as const;

/** The longest prompt kept, in bytes. */
export const PROMPT_MOST = 32 * 1024;

/** The longest name a server may have, and what it may be made of: it is
 *  part of every tool name a session lists. */
export const NAME_MOST = 32;
const NAME = /^[A-Za-z0-9_-]+$/;
const RESERVED = "fastctx";

/** Why `name` cannot name a server, or null when it can. */
export function nameProblem(name: string): "rule" | "reserved" | null {
  if (!name || name.length > NAME_MOST || !NAME.test(name)) return "rule";
  return name.toLowerCase() === RESERVED ? "reserved" : null;
}

/** A name for `name` that `taken` does not hold yet: as it is, then with
 *  what of `machine`'s name can be part of one, then numbered
 *  (`fastctx_types::profile::free_name`). */
export function freeName(name: string, machine: string, taken: (name: string) => boolean): string {
  if (!taken(name)) return name;
  const usable = machine
    .toLowerCase()
    .replace(/[^a-z0-9_-]+/g, "-")
    .split("-")
    .filter(Boolean)
    .join("-");
  const fit = (tail: string) => `${name.slice(0, Math.max(0, NAME_MOST - tail.length - 1))}-${tail}`;
  if (usable) {
    const named = fit(usable.slice(0, NAME_MOST / 2));
    if (!taken(named)) return named;
  }
  for (let n = 2; ; n += 1) {
    const named = fit(String(n));
    if (!taken(named)) return named;
  }
}

/** The servers an `mcpServers` JSON object names, as Claude Desktop, Claude
 *  Code and Cursor write it (`fastctx_types::profile::parse_servers`); the
 *  object itself or a document holding it. */
export function parseServers(text: string): { servers: [string, Server][] } | { problem: "json" | "none" | "shape"; detail?: string } {
  let value: unknown;
  try {
    value = JSON.parse(text);
  } catch (error) {
    return { problem: "json", detail: error instanceof Error ? error.message : String(error) };
  }
  if (!value || typeof value !== "object" || Array.isArray(value)) return { problem: "shape" };
  const document = value as Record<string, unknown>;
  const held = document.mcpServers ?? document.servers ?? document;
  if (!held || typeof held !== "object" || Array.isArray(held)) return { problem: "shape" };
  const servers: [string, Server][] = [];
  for (const [name, entry] of Object.entries(held as Record<string, unknown>)) {
    if (!entry || typeof entry !== "object") continue;
    const server = serverOf(entry as Record<string, unknown>);
    if (server) servers.push([name, server]);
  }
  return servers.length ? { servers } : { problem: "none" };
}

function serverOf(entry: Record<string, unknown>): Server | null {
  const text = (key: string) => (typeof entry[key] === "string" ? (entry[key] as string) : undefined);
  const strings = (key: string) => {
    const held = entry[key];
    if (!held || typeof held !== "object" || Array.isArray(held)) return undefined;
    const pairs = Object.entries(held as Record<string, unknown>).filter((pair): pair is [string, string] => typeof pair[1] === "string");
    return pairs.length ? Object.fromEntries(pairs) : undefined;
  };
  const command = text("command");
  if (command) {
    const server: Server = { kind: "command", command };
    const args = Array.isArray(entry.args) ? entry.args.filter((arg): arg is string => typeof arg === "string") : [];
    if (args.length) server.args = args;
    const env = strings("env");
    if (env) server.env = env;
    const cwd = text("cwd");
    if (cwd) server.cwd = cwd;
    return server;
  }
  const url = text("url") ?? text("serverUrl");
  if (url && /^https?:\/\//.test(url)) {
    const server: Server = { kind: "remote", url };
    if ((text("type") ?? text("transport"))?.toLowerCase() === "sse") server.sse = true;
    const headers = strings("headers");
    if (headers) server.headers = headers;
    return server;
  }
  return null;
}

export function readProfile(hub: string | null, account: string | null): Promise<ProfileView> {
  return invoke<ProfileView>("profile_read", { hub, account });
}

/** Writes what `readProfile` read, over its version; its servers are kept as
 *  they were read (they change through `mcp`). Answers the new version. */
export function writeProfile(hub: string | null, account: string | null, profile: Profile): Promise<number> {
  return invoke<number>("profile_write", { hub, account, profile });
}

export function mcp(hub: string | null, account: string | null, call: McpCall): Promise<McpReply> {
  return invoke<McpReply>("mcp", { hub, account, mcp: call });
}
