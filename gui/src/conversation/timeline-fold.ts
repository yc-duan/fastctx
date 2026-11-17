import type { Entry } from "./model";
import type { SessionEvent } from "../world/events";

/** Cache the event's own contribution, before later turn or usage facts. */
export function eventBase(event: SessionEvent, entry: Entry): Entry {
  const base = { ...entry };
  if (base.kind === "prose") {
    if (event.event === "finished" || event.event === "began" || event.event === "updated" || event.event === "grew") base.writing = event.event !== "finished";
    delete base.response;
    if ((event.event === "finished" || event.event === "began" || event.event === "updated") && event.item.kind === "thinking") {
      base.began = event.item.started_at ? Date.parse(event.item.started_at) : Date.parse(event.at);
      if (event.item.duration_ms === undefined) delete base.took;
      else base.took = event.item.duration_ms;
    }
  }
  if (base.kind === "tool" && (event.event === "finished" || event.event === "began" || event.event === "updated") && event.item.kind === "tool") {
    const { kind: _kind, id: _id, started_at: _started, detail: _detail, ...tool } = event.item;
    base.tool = tool;
    base.began = event.item.started_at ? Date.parse(event.item.started_at) : Date.parse(event.at);
    const use = tool.status === "running" ? null : tool.status === "unknown" ? "unknown" : tool.status;
    base.ended = event.event === "finished" && use ? { at: Date.parse(event.at), use } : null;
  }
  return base;
}

/** Entries share immutable payloads; compare those references before walking
 * small derived fields. Display clocks and local ordinals are not content. */
export function sameProjection(left: Entry, right: Entry): boolean {
  const equal = (a: unknown, b: unknown): boolean => {
    if (a === b) return true;
    if (!a || !b || typeof a !== "object" || typeof b !== "object") return false;
    if (Array.isArray(a) !== Array.isArray(b)) return false;
    const keys = Object.keys(a);
    return keys.length === Object.keys(b).length && keys.every((key) => equal((a as Record<string, unknown>)[key], (b as Record<string, unknown>)[key]));
  };
  const { stamp: _a, at: _b, ...a } = left;
  const { stamp: _c, at: _d, ...b } = right;
  return equal(a, b);
}
