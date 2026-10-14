// Each model request of a conversation, as its host recorded what it cost
// (`world-gui.md`, 对话): how much of the prompt the provider's cache served,
// how much was written to it and for how long, and what came back. It is
// read from the events the conversation already holds — Claude Code's
// session file, read on its own machine, and Codex's usage after every
// response — so nothing is asked of any machine or provider for it.

import type { JsonValue } from "../world/wire";

export interface ModelRequest {
  /** When its answer was written, in milliseconds. */
  at: number;
  /** The prompt in three parts: sent afresh, read from the cache, written
   *  to it. Together they are the context the request was answered over. */
  fresh: number;
  read: number;
  write: number;
  output: number;
  /** How long what it wrote stays in the cache, where the host says;
   *  reading what is there keeps it as long again. */
  ttl: number | null;
  /** The turn it was made in, where the host says. */
  turn: string | null;
  /** What its answer holds, where the host says: Claude Code names each
   *  block, a tool call by the call's id, so the calls a request made are
   *  known; Codex names none, and its calls are placed by time. */
  items: string[];
}

/** The context a request was answered over, and the share the cache
 *  served of it. */
export const contextOf = (request: ModelRequest): number => request.fresh + request.read + request.write;
export const hitOf = (request: ModelRequest): number => {
  const context = contextOf(request);
  return context ? request.read / context : 0;
};

const MINUTE = 60_000;

export class Requests {
  private readonly byKey = new Map<string, ModelRequest>();
  /** Moves whenever a request is added or told again. */
  version = 0;

  /** A row of Claude Code's session file: every row of one response says
   *  the response's usage again, the last most whole. */
  claude(at: number, response: string | null, usage: JsonValue, turn: string | null = null, items: readonly string[] = []): void {
    const read = count(usage, "cache_read_input_tokens");
    const write = count(usage, "cache_creation_input_tokens");
    const fresh = count(usage, "input_tokens");
    if (read === null || write === null || fresh === null) return;
    const buckets = field(usage, "cache_creation");
    const hour = count(buckets, "ephemeral_1h_input_tokens") ?? 0;
    const five = count(buckets, "ephemeral_5m_input_tokens") ?? 0;
    const ttl = hour > 0 ? 60 * MINUTE : five > 0 ? 5 * MINUTE : null;
    const key = `claude:${response ?? at}`;
    // Each row of a response is one block of its answer: the blocks add up.
    const known = this.byKey.get(key)?.items ?? [];
    const held = [...known, ...items.filter((item) => !known.includes(item))];
    this.byKey.set(key, { at, fresh, read, write, output: count(usage, "output_tokens") ?? 0, ttl, turn, items: held });
    this.version += 1;
  }

  /** Codex's usage after a response: `last` is that response's own; a
   *  total told again, as on joining a thread, is the same response. */
  codex(at: number, usage: JsonValue, turn: string | null = null): void {
    const total = count(field(usage, "total"), "totalTokens");
    const last = field(usage, "last");
    const input = count(last, "inputTokens");
    const cached = count(last, "cachedInputTokens");
    if (total === null || input === null || cached === null) return;
    const key = `codex:${total}`;
    if (this.byKey.has(key)) return;
    // Codex counts what the cache served, and any it wrote, within its input.
    const write = count(last, "cacheWriteInputTokens") ?? 0;
    this.byKey.set(key, { at, fresh: Math.max(0, input - cached - write), read: cached, write, output: count(last, "outputTokens") ?? 0, ttl: null, turn, items: [] });
    this.version += 1;
  }

  /** Every request, keyed so the same one told twice counts once. */
  entries(): IterableIterator<[string, ModelRequest]> {
    return this.byKey.entries();
  }

  get size(): number {
    return this.byKey.size;
  }
}

/** The requests of several parts of a conversation, in order. */
export function inOrder(parts: readonly Requests[]): ModelRequest[] {
  const all = new Map<string, ModelRequest>();
  for (const part of parts) for (const [key, request] of part.entries()) all.set(key, request);
  return [...all.values()].sort((a, b) => a.at - b.at);
}

/** Until when the provider keeps the cache the last request left, where
 *  the host says how long it keeps what it writes: from the last answer,
 *  as long as the last request that wrote anything asked. */
export function warmUntil(requests: readonly ModelRequest[]): { until: number; ttl: number } | null {
  const last = requests[requests.length - 1];
  const ttl = [...requests].reverse().find((request) => request.ttl !== null)?.ttl ?? null;
  return last && ttl ? { until: last.at + ttl, ttl } : null;
}

function field(value: JsonValue | undefined | null, name: string): JsonValue | null {
  return value !== null && typeof value === "object" && !Array.isArray(value) ? (value[name] ?? null) : null;
}

function count(value: JsonValue | undefined | null, name: string): number | null {
  const found = field(value, name);
  return typeof found === "number" && Number.isFinite(found) && found >= 0 ? found : null;
}
