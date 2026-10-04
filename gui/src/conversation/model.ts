// A conversation as it reads: the session's events folded into the things
// shown, in the order they first appeared. The same reading as the
// terminal's (`crates/fastctx-tui/src/transcript.rs`), so every interface
// shows a session alike.

import type { Health, MachineName, SessionChange, SessionEvent, Speaker, Step, Tool, TurnEnd } from "../world/events";
import type { JsonValue } from "../world/wire";
import { askKey, forPeople, type AskState } from "./ask-state";

export type Use = "done" | "failed" | "declined" | "cut" | "unknown";

export type SendState =
  | { state: "going" }
  /** In the World, not yet in a turn: `how` is the way they landed
   *  (`waiting`, `sent`, `uncertain`), as the node said. */
  | { state: "landed"; how: string; machine: MachineName; why?: string | null }
  | { state: "failed"; why: string };

/** Something the user attached to their words, as it is shown with them. */
export interface ShownAttachment {
  name: string;
  size: number | null;
  image: boolean;
  preview: string | null;
}

export type Entry =
  | { kind: "heard"; key: string; from: Speaker | null; text: string; turn: string | null; sending?: SendState; attached?: ShownAttachment[] }
  | {
      kind: "prose";
      key: string;
      item: string;
      turn: string;
      prose: "message" | "thinking" | "plan";
      text: string;
      writing: boolean;
      /** Bumped whenever the text changes, so a view redraws only what did. */
      version: number;
    }
  | { kind: "tool"; key: string; item: string; turn: string; tool: Tool; began: number; ended: { at: number; use: Use } | null }
  | { kind: "steps"; key: string; turn: string; steps: Step[]; note: string | null }
  | { kind: "compaction"; key: string; item: string; turn: string }
  | { kind: "other"; key: string; item: string; turn: string; hostKind: string }
  | { kind: "activity"; key: string; item: string; turn: string; text: string }
  | { kind: "ended"; key: string; turn: string; end: TurnEnd }
  | { kind: "health"; key: string; health: Health }
  | { kind: "uncertain"; key: string; why: string }
  /** A request the session waits on the user for, as its host made it. */
  | { kind: "ask"; key: string; id: JsonValue; turn: string | null; request: string; params: JsonValue; state: AskState; version: number };

/** One stretch of what a turn did between things it said: tools and
 *  thinking in a row, folded under one heading. */
export interface Activity {
  kind: "activity-group";
  key: string;
  turn: string;
  members: Entry[];
}

export type Block = Entry | Activity;

type Keyed = Extract<Entry, { item: string; turn: string }>;

export class Conversation {
  entries: Entry[] = [];
  context: { used: number; window: number | null } | null = null;
  title: string | null = null;
  /** The host replaced or removed turns: what is shown must be read again. */
  stale = false;
  private serial = 0;

  private key(prefix: string): string {
    this.serial += 1;
    return `${prefix}-${this.serial}`;
  }

  /** What came before, replacing what is shown. */
  history(events: SessionEvent[]): void {
    // Requests still waiting are kept: the host asked them, and they are
    // not part of the history it gives back.
    const waiting = this.entries.filter((entry) => entry.kind === "ask" && entry.state.state === "waiting");
    this.entries = [];
    this.stale = false;
    for (const event of events) this.event(event);
    for (const ask of waiting) {
      if (ask.kind === "ask" && !this.entries.some((entry) => entry.kind === "ask" && askKey(entry.id) === askKey(ask.id))) this.entries.push(ask);
    }
  }

  event(event: SessionEvent): void {
    const at = Date.parse(event.at);
    switch (event.event) {
      case "turn_started":
      case "realtime":
      case "realtime_grew":
        return;
      case "heard": {
        // Words sent from here are now heard: they keep their place.
        if (event.from?.kind === "user") {
          const sent = this.entries.find(
            (entry) => entry.kind === "heard" && entry.sending !== undefined && entry.text === event.text,
          );
          if (sent && sent.kind === "heard") {
            delete sent.sending;
            sent.turn = event.turn;
            sent.from = event.from;
            return;
          }
        }
        const known = this.entries.some(
          (entry) => entry.kind === "heard" && entry.text === event.text && event.turn !== null && entry.turn === event.turn,
        );
        if (!known) {
          this.entries.push({ kind: "heard", key: this.key("heard"), from: event.from, text: event.text, turn: event.turn });
        }
        return;
      }
      case "began":
      case "updated":
        this.item(event.turn, event.item, at, false);
        return;
      case "finished":
        this.item(event.turn, event.item, at, true);
        return;
      case "grew": {
        const shown = this.find(event.turn, event.item);
        if (shown?.kind === "prose" && event.part === "text") {
          shown.text += event.text;
          shown.version += 1;
        } else if (!shown && event.part === "text") {
          // A message's first words can come before any word of it.
          this.entries.push({
            kind: "prose",
            key: this.key("prose"),
            item: event.item,
            turn: event.turn,
            prose: "message",
            text: event.text,
            writing: true,
            version: 0,
          });
        }
        return;
      }
      case "steps": {
        const shown = [...this.entries].reverse().find((entry) => entry.kind === "steps" && entry.turn === event.turn);
        if (shown?.kind === "steps") {
          shown.steps = event.steps;
          shown.note = event.note;
        } else {
          this.entries.push({ kind: "steps", key: this.key("steps"), turn: event.turn, steps: event.steps, note: event.note });
        }
        return;
      }
      case "context":
        this.context = { used: event.used, window: event.window };
        return;
      case "titled":
        this.title = event.title;
        return;
      case "turn_ended": {
        if (this.entries.some((entry) => entry.kind === "ended" && entry.turn === event.turn)) return;
        for (const entry of this.entries) {
          if (entry.kind === "tool" && entry.turn === event.turn && entry.ended === null) entry.ended = { at, use: "cut" };
          if (entry.kind === "prose" && entry.turn === event.turn) entry.writing = false;
        }
        this.entries.push({ kind: "ended", key: this.key("ended"), turn: event.turn, end: event.end });
        return;
      }
      case "delivery_uncertain":
        this.entries.push({ kind: "uncertain", key: this.key("uncertain"), why: event.why });
        return;
      case "health":
        this.health(event.health);
        return;
      case "changed": {
        const change = event.change;
        if (change.kind === "request") {
          if (!forPeople(change.request) || this.ask(change.id)) return;
          this.entries.push({
            kind: "ask",
            key: this.key("ask"),
            id: change.id,
            turn: event.turn,
            request: change.request,
            params: change.params,
            state: { state: "waiting" },
            version: 0,
          });
          return;
        }
        if (change.kind === "resolved") {
          const ask = this.ask(change.id);
          if (ask && ask.state.state !== "answered") this.settle(ask, { state: "gone" });
          return;
        }
        if (change.kind === "history_changed") {
          this.stale = true;
          return;
        }
        const health = changeHealth(change);
        if (health) this.health(health);
        return;
      }
    }
  }

  private ask(id: JsonValue): Extract<Entry, { kind: "ask" }> | undefined {
    const key = askKey(id);
    return this.entries.find((entry): entry is Extract<Entry, { kind: "ask" }> => entry.kind === "ask" && askKey(entry.id) === key);
  }

  /** How a request now stands: being answered, answered, or no longer
   *  waited on. */
  settle(ask: Extract<Entry, { kind: "ask" }>, state: AskState): void {
    ask.state = state;
    ask.version += 1;
  }

  /** Whether the session waits on the user for anything. */
  waiting(): boolean {
    return this.entries.some((entry) => entry.kind === "ask" && entry.state.state === "waiting");
  }

  /** Something of the session's own health, shown once however often the
   *  host repeats it in a row. */
  private health(health: Health): void {
    const last = this.entries[this.entries.length - 1];
    if (last?.kind === "health" && last.health.kind === health.kind && last.health.text === health.text) return;
    this.entries.push({ kind: "health", key: this.key("health"), health });
  }

  /** An item began or finished: shown where it first appeared, as it now
   *  stands. */
  private item(turn: string, native: import("../world/events").Item, at: number, finished: boolean): void {
    const startedAt = native.started_at ? Date.parse(native.started_at) : at;
    const id = native.id;
    let fresh: Entry;
    switch (native.kind) {
      case "message":
      case "thinking":
      case "plan":
        fresh = {
          kind: "prose",
          key: this.key("prose"),
          item: id,
          turn,
          prose: native.kind,
          text: native.text,
          writing: !finished,
          version: 0,
        };
        break;
      case "agent_activity":
        fresh = { kind: "activity", key: this.key("activity"), item: id, turn, text: `${native.path}: ${native.activity}` };
        break;
      case "tool": {
        const { kind: _kind, id: _id, started_at: _started, detail: _detail, ...tool } = native;
        const use: Use | null =
          tool.status === "running" ? null : tool.status === "unknown" ? "unknown" : (tool.status as Use);
        fresh = {
          kind: "tool",
          key: this.key("tool"),
          item: id,
          turn,
          tool,
          began: startedAt,
          ended: finished && use ? { at, use } : null,
        };
        break;
      }
      case "compaction":
        // Shown once it is done: the context it leaves is what counts.
        if (!finished) return;
        fresh = { kind: "compaction", key: this.key("compaction"), item: id, turn };
        break;
      case "hidden":
        // A completed item can remove a transient placeholder.
        if (id) this.entries = this.entries.filter((entry) => !("item" in entry && entry.item === id && entry.turn === turn));
        return;
      case "other":
        if (!finished) return;
        fresh = { kind: "other", key: this.key("other"), item: id, turn, hostKind: native.host_kind };
        break;
    }
    if (!id) {
      this.entries.push(fresh);
      return;
    }
    const shown = this.find(turn, id);
    if (!shown) {
      this.entries.push(fresh);
      return;
    }
    if (shown.kind === "prose" && fresh.kind === "prose") {
      // A message still being written keeps what it has until its whole
      // text comes.
      if (shown.writing && !finished && fresh.text === "") return;
      if (shown.text !== fresh.text) shown.version += 1;
      shown.text = fresh.text;
      shown.writing = fresh.writing;
      return;
    }
    if (shown.kind === "tool" && fresh.kind === "tool") {
      shown.tool = fresh.tool;
      shown.ended = fresh.ended;
      return;
    }
    const index = this.entries.indexOf(shown);
    this.entries[index] = { ...fresh, key: shown.key };
  }

  private find(turn: string, id: string): Keyed | undefined {
    return this.entries.find((entry): entry is Keyed => "item" in entry && entry.item === id && entry.turn === turn);
  }

  /** Words the user is sending, shown at once; returns their key. */
  sending(text: string, here: MachineName | null, attached: ShownAttachment[] = []): string {
    const key = this.key("heard");
    this.entries.push({
      kind: "heard",
      key,
      from: here ? { kind: "user", machine: here } : null,
      text,
      turn: null,
      sending: { state: "going" },
      ...(attached.length ? { attached } : {}),
    });
    return key;
  }

  /** What became of words being sent: they are kept, with how they stand,
   *  until the session hears them. */
  sent(key: string, state: SendState): void {
    const entry = this.entries.find((candidate) => candidate.key === key);
    if (entry?.kind === "heard" && entry.sending) entry.sending = state;
  }

  /** Whether anything shown is happening now. */
  moving(): boolean {
    return this.entries.some(
      (entry) =>
        (entry.kind === "tool" && entry.ended === null) ||
        (entry.kind === "prose" && entry.writing) ||
        (entry.kind === "heard" && entry.sending?.state === "going"),
    );
  }

  /** The entries as they are laid out: what a turn did between the things
   *  it said, folded into activity groups. Thinking belongs to the group
   *  it sits in; words in between are prose and divide groups. */
  blocks(): Block[] {
    const blocks: Block[] = [];
    let group: Activity | null = null;
    for (const entry of this.entries) {
      const activity =
        entry.kind === "tool" ||
        entry.kind === "activity" ||
        (entry.kind === "prose" && entry.prose === "thinking");
      if (activity && "turn" in entry) {
        if (!group || group.turn !== entry.turn) {
          group = { kind: "activity-group", key: `group-${entry.key}`, turn: entry.turn, members: [] };
          blocks.push(group);
        }
        group.members.push(entry);
      } else {
        group = null;
        blocks.push(entry);
      }
    }
    return blocks;
  }
}

/** What a host's state change says of the session's health: the same
 *  mapping as `SessionChange::health()`. */
export function changeHealth(change: SessionChange): Health | null {
  switch (change.kind) {
    case "rerouted":
      return {
        kind: "rerouted",
        text: `OpenAI moved this turn from ${change.from} to ${change.to} (${change.reason})`,
      };
    case "safety": {
      return property(change.buffering, "showBufferingUi") === true
        ? { kind: "safety", text: "OpenAI is holding this turn's output for extra safety processing" }
        : null;
    }
    case "error": {
      const message = property(change.error, "message");
      if (typeof message !== "string") return null;
      return { kind: change.will_retry ? "reconnecting" : "error", text: message };
    }
    case "tool_server":
      return change.status === "failed"
        ? { kind: "tool_server", text: `its ${change.server} tools did not start: ${typeof change.error === "string" ? change.error : "no reason given"}` }
        : null;
    case "warning":
      return { kind: "warning", text: change.text };
    default:
      return null;
  }
}

function property(value: JsonValue, key: string): JsonValue | undefined {
  return value !== null && typeof value === "object" && !Array.isArray(value) ? value[key] : undefined;
}
