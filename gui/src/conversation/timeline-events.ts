// Keep one bounded snapshot per live identity, never a journal of chunks.
import type { SessionEvent } from "../world/events";
import type { Window } from "./conversation";

export function placeEvent(window: Window, key: string, after: string | null): boolean {
  const pages = window.pages;
  if (!pages?.keys) return false;
  const objects = new Map(pages.objects.map((object) => [pages.keys![object.at]!, object]));
  const positions = new Map(pages.keys.map((key, at) => [key, pages.places?.[at] ?? null]));
  const at = pages.keys.indexOf(key);
  if (at < 0) return false;
  const before = after === null ? -1 : pages.keys.indexOf(after);
  if (after !== null && (before < 0 || after === key)) return false;
  const event = window.events[at]!;
  pages.keys.splice(at, 1); window.events.splice(at, 1);
  const insert = before + 1 - (before > at ? 1 : 0);
  pages.keys.splice(insert, 0, key); window.events.splice(insert, 0, event);
  pages.places = pages.keys.map((key) => positions.get(key) ?? null);
  pages.objects = pages.keys.flatMap((key, at) => { const object = objects.get(key); return object ? [{ ...object, at }] : []; });
  return true;
}

export function grown(previous: SessionEvent | undefined, event: SessionEvent): SessionEvent | undefined {
  if (event.event !== "grew") return event;
  if (previous && (previous.event === "began" || previous.event === "updated")) {
    const snapshot = { ...previous, item: { ...previous.item } };
    const what = snapshot.item;
    if (event.part === "text" && (what.kind === "message" || what.kind === "thinking" || what.kind === "plan")) what.text += event.text;
    else if (event.part === "output" && what.kind === "tool") {
      what.output = [...what.output];
      const last = what.output.at(-1);
      if (last?.type === "text") what.output[what.output.length - 1] = { ...last, text: last.text + event.text };
      else what.output.push({ type: "text", text: event.text });
    }
    return snapshot;
  }
  return undefined;
}
