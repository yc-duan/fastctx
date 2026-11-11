import { sameEvent } from "./event-identity";
import { grown, placeEvent } from "./timeline-events";
import { canReviseItem } from "./timeline-fold";
// A conversation as its page holds it (`world-gui.md`, 对话: 很长的对话). The
// shell keeps every event of it (`src-tauri/src/transcript.rs`); the page
// holds its latest turns, read as they go on, and whatever earlier parts its
// reader came to, each folded on its own (`model.ts`). Turns not held stand
// as gaps, before everything held or between two parts, read from the shell
// when the reader comes to them and let go again when the reader is far
// away. So however long a conversation grows, the page holds and draws a few
// thousand events of it at most, and an event costs it the same.
//
// Everything a page asks of a conversation is asked here, as of one: what
// it lays out, an entry by its key, what a turn said, whether it is busy.
// Each question goes to the part that holds its answer; what is live (the
// turn under way, words being sent, requests waiting) is in the latest.

import { askKey, type AskState } from "./ask-state";
import { inOrder, type ModelRequest } from "./requests";
import type { SessionEvent, TurnEnd } from "../world/events";
import type { MachineName } from "../world/events";
import { Fold, sayOf, type Block, type Entry, type Gap, type KeptWords, type SendState, type Settler, type ShownAttachment } from "./model";
import type { AgentRun } from "./agents";
import { machineNamed } from "./acted-on";
import type { PendingControl } from "../world/control";

export interface TimelinePosition { order: string; item: string }

/** The part a page opens on, in place of all it held. */
export interface Window {
  generation: number;
  /** Where its events begin, among all the conversation's. */
  start: number;
  turns_before: number;
  events: SessionEvent[];
  /** What its turns read of what came before them. */
  carry: SessionEvent[];
  /** Opaque timeline paging has no full-history ordinal or turn count. */
  pages?: { older: boolean; newer: boolean; preparing?: boolean; partial_older?: boolean; partial_newer?: boolean; keys?: string[]; places?: (TimelinePosition | null)[]; live_keys?: string[]; pending?: SessionEvent[]; objects: { at: number; bytes: number; object: string }[] };
}

export interface TimelineDelta {
  generation: number;
  events: { key: string; event: SessionEvent; place?: TimelinePosition | null }[];
  pending?: SessionEvent[];
  removed: string[];
  placed: { key: string; after: string | null }[];
  pages: NonNullable<Window["pages"]>;
}

/** A part read from the shell. */
export interface PartRead {
  generation: number;
  start: number;
  end: number;
  turns_before: number;
  turns: number;
  events: SessionEvent[];
  /** Facts its runs told after it. */
  late: SessionEvent[];
  /** A bounded replacement, when the shell reads directly from the node. */
  window?: Window;
}

export type Edge =
  | { edge: "latest" }
  | { edge: "position"; place: TimelinePosition }
  | { edge: "turn"; turn: string }
  | { edge: "before"; at: number; floor: number }
  | { edge: "after"; at: number; ceiling: number }
  | { edge: "around"; at: number; floor: number; ceiling: number };

export interface Cut {
  generation: number;
  at: number;
  turns_before: number;
}

/** Displayed text of one part of a block, lowercased as finding compares,
 *  and the event its block begins at. */
export interface FindDoc {
  block: string;
  part: string;
  text: string;
  at: number;
}

export interface Found {
  generation: number;
  /** Matches in what the page does not hold: each part's block, the part,
   *  how many, where its block begins. */
  ranges: { start: number; end: number; hits: [string, string, number, number][] }[];
  unindexed: [number, number][];
}

/** Where the parts a page does not hold are read from. */
export interface ScanCursor { revision: string; after: TimelinePosition; until: TimelinePosition; through: string | null; seeking?: boolean; restarts?: number }
export interface ExpandedEvent { object: string; event: SessionEvent }
export interface Scanned { events: SessionEvent[]; truncated?: { at: number; bytes: number }[]; restart?: boolean; next: ScanCursor | null }
export type ExportReading = readonly Entry[] | { previewBytes: number } | { restart: true };

export interface PartSource {
  identity?(): string;
  search?(request: import("./timeline-find").SearchRequest): Promise<import("./timeline-find").SearchReply>;
  scan?(cursor: ScanCursor | null): Promise<Scanned>;
  part(generation: number, edge: Edge): Promise<PartRead | null>;
  object?(generation: number, object: string): Promise<ExpandedEvent>;
  follow?(revision: number, following: boolean): Promise<void>;
  /** Where a turn first appears. */
  locate(generation: number, turn: string): Promise<number | null>;
  cut(generation: number, events: number): Promise<Cut | null>;
  index(generation: number, start: number, end: number, docs: FindDoc[]): Promise<boolean>;
  find(generation: number, query: string, held: [number, number][]): Promise<Found | null>;
}

/** A part held: the events `start..end` of the conversation, folded. */
export interface Held {
  start: number;
  end: number;
  turnsBefore: number;
  turns: number;
  fold: Fold;
}

type Heard = Extract<Entry, { kind: "heard" }>;
type ProseEntry = Extract<Entry, { kind: "prose" }>;
type ToolEntry = Extract<Entry, { kind: "tool" }>;


/** How long after one bubble the next has its moment said above it. */
const CHAT_GAP_MS = 5 * 60_000;
export class Conversation {
  searchVersion = 0;
  private searchSource = 0;
  get searchIdentity(): string { return this.source?.identity?.() ?? String(this.searchSource); }
  search(request: import("./timeline-find").SearchRequest): Promise<import("./timeline-find").SearchReply> {
    if (!this.source?.search) return Promise.reject(new Error("this source cannot search the complete history"));
    return this.source.search(request);
  }
  /** The parts held, in order; the last is the latest, and live. */
  private parts: Held[];
  private generation = 0;
  private pageWindow: Window | null = null;
  private expanded = new Map<string, SessionEvent>();
  private terminalBodies = new Map<string, string>();
  private followRevision = 0;
  private follows: boolean | null = null;
  private source: PartSource | null = null;
  /** Moves when parts come and go. */
  private structure = 0;
  private folds = 0;
  private cache: { version: string; blocks: Block[] } | null = null;
  private chatCache: { version: string; blocks: Block[] } | null = null;
  /** Gaps being read, by key. */
  private reading = new Set<string>();
  private readingFailure: string | null = null;
  takeReadingFailure(): string | null {
    const failure = this.readingFailure;
    this.readingFailure = null;
    return failure;
  }
  /** Folds read aside, as finding reads what the page does not hold:
   *  asked as the others are while they are read. */
  private aside: Fold[] = [];
  private thoughts = true;
  /** Something read from the shell arrived: the page draws again. */
  changed: () => void = () => {};

  constructor() {
    this.parts = [{ start: 0, end: 0, turnsBefore: 0, turns: 0, fold: new Fold("t") }];
  }

  private get tail(): Held {
    return this.parts[this.parts.length - 1]!;
  }

  /** The latest part's fold: what is live. */
  get live(): Fold {
    return this.tail.fold;
  }

  // What is read of the latest part, as the page shows it.
  get title(): string | null {
    return this.live.title;
  }
  /** Shown at once as the user renames it, until the host says. */
  set title(title: string | null) {
    this.live.title = title;
  }
  get context(): { used: number; window: number | null; at: number } | null {
    return this.live.context;
  }
  get answeredBy(): string | null {
    return this.live.answeredBy;
  }
  /** Every model request of the parts held, in order; put together again
   *  only when a part's requests change or parts come and go. */
  get requests(): ModelRequest[] {
    const key = this.parts.map((part) => `${part.start}:${part.fold.requests.version}`).join(",");
    if (this.requestsHeld?.key !== key) this.requestsHeld = { key, list: inOrder(this.parts.map((part) => part.fold.requests)) };
    return this.requestsHeld.list;
  }
  private requestsHeld: { key: string; list: ModelRequest[] } | null = null;
  get stale(): boolean {
    return this.live.stale;
  }
  /** Moves whenever one of the host's own agents does, in any part. */
  get agentStamp(): number {
    let stamp = 0;
    for (const part of this.parts) stamp += part.fold.agentStamp;
    return stamp;
  }
  /** Whether its thinking's words are kept (`Fold.thoughtWords`). */
  get thoughtWords(): boolean {
    return this.thoughts;
  }
  set thoughtWords(kept: boolean) {
    this.thoughts = kept;
    for (const part of this.parts) part.fold.thoughtWords = kept;
  }

  /** The parts held, in order. */
  held(): readonly Held[] {
    return this.parts;
  }

  /** The part a block is laid out in. */
  partOf(key: string): Held | null {
    for (const part of this.parts) if (part.fold.get(key) || part.fold.blocks().some((block) => block.key === key)) return part;
    return null;
  }

  /** The bounds of every part held, as finding asks the shell. */
  bounds(): [number, number][] {
    return this.parts.map((part) => [part.start, part.end]);
  }

  /** Whether parts of it are read from a shell. */
  get preparing(): boolean { return this.pageWindow?.pages?.preparing === true; }

  get windowed(): boolean {
    return this.source !== null;
  }

  // Reading it from the shell.

  /** The part the page opens on, in place of all it held. */
  open(window: Window, source: PartSource | null): void {
    this.searchVersion += 1;
    if (source !== this.source) this.searchSource += 1;
    this.generation = window.generation;
    this.pageWindow = window.pages ? window : null;
    this.follows = null;
    this.terminalBodies.clear();
    const objects = new Set(window.pages?.objects.map((object) => object.object));
    for (const object of this.expanded.keys()) if (!objects.has(object)) this.expanded.delete(object);
    this.source = source;
    const tail = this.live;
    const events = [...window.events];
    for (const object of window.pages?.objects ?? []) {
      const expanded = this.expanded.get(object.object);
      if (expanded) events[object.at] = expanded;
    }
    tail.history(events, window.carry, window.pages?.keys, !window.pages, window.pages?.pending);
    this.parts = [{ start: window.start, end: window.start + window.events.length, turnsBefore: window.turns_before, turns: 0, fold: tail }];
    this.reading.clear();
    this.structure += 1;
  }

  /** Events kept after all that was given; false when they do not follow
   *  what is held, and it must be read again. */
  extended(start: number, events: SessionEvent[]): boolean {
    if (start !== this.tail.end) return false;
    this.live.extend(events);
    this.tail.end += events.length;
    return true;
  }

  /** One thing as it happened, the `at`th event; false when it does not
   *  follow what is held. */
  happened(at: number, event: SessionEvent): boolean {
    if (at !== this.tail.end) return false;
    this.route(event);
    this.live.event(event);
    this.pageWindow?.events.push(event);
    this.tail.end += 1;
    return true;
  }

  /** Applies only the realtime change, retaining no raw chunk journal. */
  liveEvent(event: SessionEvent, key: string, evicted: readonly string[] = [], placement?: { after: string | null }, older?: boolean, object?: string, body?: SessionEvent): boolean {
    if (event.event === "changed" && (event.change.kind === "request" || event.change.kind === "resolved")) {
      this.live.livePending(event);
      const pages = this.pageWindow?.pages;
      if (pages) {
        const id = askKey(event.change.id);
        const pending = (pages.pending ?? []).filter((event) => event.event !== "changed" || event.change.kind !== "request" || askKey(event.change.id) !== id);
        if (event.change.kind === "request") pending.push(event);
        pages.pending = pending;
      }
      return true;
    }
    const held = this.pageWindow;
    const oldPlaces = new Map(held?.pages?.keys?.map((key, at) => [key, held.pages?.places?.[at] ?? null]));
    if (held?.pages?.keys) {
      if (older !== undefined && held.pages.older !== older) { held.pages.older = older; this.structure += 1; }
      const at = held.pages.keys.indexOf(key);
      const snapshot = grown(at < 0 ? undefined : held.events[at], event);
      if (!snapshot) return false;
      if (at < 0) { held.pages.keys.push(key); held.events.push(snapshot); held.pages.places?.push(null); }
      else { held.events[at] = snapshot; held.pages.objects = held.pages.objects.filter((object) => object.at !== at); }
    }
    const cached = new Set(this.expanded.keys());
    if (object && body) { this.terminalBodies.set(object, key); this.rememberBody(object, body); }
    this.route(event);
    this.live.event(body ?? event, key);
    if (held?.pages?.keys && [...cached].some((object) => !this.expanded.has(object))) {
      const bodies = new Map([...this.terminalBodies].map(([object, key]) => [key, this.expanded.get(object)!]));
      for (const object of held.pages.objects) { const full = this.expanded.get(object.object); if (full) bodies.set(held.pages.keys[object.at]!, full); }
      const shown = held.events.map((event, at) => bodies.get(held.pages!.keys![at]!) ?? event);
      const fold = this.live.revised(held.events, held.pages.keys, shown, held.pages.keys, held.pages.pending ?? []);
      this.parts = [{ ...this.tail, fold }];
    }
    const removed = new Set(evicted);
    this.live.evictPlaces([...removed]);
    if (removed.size) {
      const window = this.pageWindow;
      if (window?.pages?.keys) {
        const positions = new Map<number, number>();
        const keys: string[] = [];
        window.events = window.events.filter((_, at) => {
          if (removed.has(window.pages!.keys![at]!)) return false;
          positions.set(at, keys.length); keys.push(window.pages!.keys![at]!); return true;
        });
        window.pages.keys = keys;
        window.pages.places = keys.map((current) => oldPlaces.get(current) ?? (current === key ? evicted.map((old) => oldPlaces.get(old)).find((place) => !!place) : null) ?? null);
        window.pages.objects = window.pages.objects.flatMap((object) => { const at = positions.get(object.at); return at === undefined ? [] : [{ ...object, at }]; });

        this.tail.end = window.events.length;
      }
      this.parts = [...this.parts]; this.structure += 1;
    }
    if (placement && this.pageWindow && !placeEvent(this.pageWindow, key, placement.after)) return false;
    return true;
  }

  /** Materialization reuses unchanged events and preserves the navigation. */
  timeline(delta: TimelineDelta): boolean {
    this.searchVersion += 1;
    const held = this.pageWindow;
    if (!held?.pages?.keys || held.generation !== delta.generation) return false;
    const events = new Map(held.pages.keys.map((key, at) => [key, held.events[at]!]));
    const places = new Map(held.pages.keys.map((key, at) => [key, held.pages?.places?.[at] ?? null]));
    for (const changed of delta.events) { events.set(changed.key, changed.event); places.set(changed.key, changed.place ?? places.get(changed.key) ?? null); }
    const removed = new Set(delta.removed);
    const keys = held.pages.keys.filter((key) => !removed.has(key));
    for (const place of delta.placed) {
      const old = keys.indexOf(place.key);
      if (old >= 0) keys.splice(old, 1);
      const at = place.after === null ? -1 : keys.indexOf(place.after);
      if (place.after !== null && at < 0) return false;
      keys.splice(at + 1, 0, place.key);
    }
    const ordered = keys.map((key) => events.get(key));
    if (ordered.some((event) => event === undefined)) return false;
    const window = { ...held, events: ordered as SessionEvent[], pages: { ...delta.pages, keys, places: keys.map((key) => places.get(key) ?? null), pending: delta.pending ?? held.pages.pending ?? [] } };
    const objects = new Set(window.pages.objects.map((object) => object.object));
    for (const [object, key] of this.terminalBodies) if (objects.has(object) || !keys.includes(key)) this.terminalBodies.delete(object);
    for (const object of this.expanded.keys()) if (!objects.has(object) && !this.terminalBodies.has(object)) this.expanded.delete(object);
    const shown = [...window.events];
    for (const object of window.pages.objects) { const full = this.expanded.get(object.object); if (full) shown[object.at] = full; }
    const previous = new Map(held.pages.keys.map((key, at) => [key, held.events[at]]));
    const hasUsage = shown.some((event) => event.event === "changed" && (event.change.kind === "usage" || event.change.kind === "message_usage"));
    const itemOnly = delta.pending === undefined && delta.removed.length === 0
      && delta.events.every(({ key, event }) => canReviseItem(previous.get(key), event, hasUsage));
    let fold = this.live;
    if (itemOnly) fold.reviseItems(shown, keys, delta.events.map(({ key }) => key), delta.removed);
    else fold = fold.revised(held.events, held.pages.keys, shown, keys, window.pages.pending);
    this.pageWindow = window;
    this.parts = [{ ...this.tail, fold, end: window.start + window.events.length }];
    this.structure += 1;
    return true;
  }

  /** Fetches only after the reader explicitly opens a deferred event. */
  async readFull(key: string): Promise<void> {
    const entry = this.get(key);
    const window = this.pageWindow;
    const source = this.source;
    const object = entry && this.objectOf(entry);
    if (!object || !source?.object || !window) return;
    const generation = this.generation;
    const expected = window.events[object.at];
    const expanded = await source.object(generation, object.object);
    if (expanded.object !== object.object) throw new Error("the expanded object did not match the selected content");
    const event = expanded.event;
    if (expected && !sameEvent(expected, event)) throw new Error("the expanded event did not match the selected entry");
    if (source !== this.source || !this.pageWindow) return;
    const current = this.pageWindow.pages?.objects.find((held) => held.object === object.object);
    if (!current) return;
    // Expanded bodies are budgeted separately within the held window. Bodies
    // evicted by paging are fetched again after the reader returns.
    this.rememberBody(current.object, event);
    const held = this.pageWindow;
    const keys = held.pages?.keys;
    if (!keys) return;
    const shown = [...held.events];
    for (const object of held.pages?.objects ?? []) {
      const full = this.expanded.get(object.object);
      if (full) shown[object.at] = full;
    }
    const fold = this.live.revised(held.events, keys, shown, keys, held.pages?.pending ?? []);
    this.parts = [{ ...this.tail, fold }];
    this.structure += 1;
    this.changed();
  }

  private rememberBody(object: string, event: SessionEvent): void {
    this.expanded.set(object, event);
    let bytes = [...this.expanded.values()].reduce((sum, e) => sum + JSON.stringify(e).length * 2, 0);
    while (bytes > 8 * 1024 * 1024 && this.expanded.size > 1) {
      const first = this.expanded.keys().next().value!;
      bytes -= JSON.stringify(this.expanded.get(first)).length * 2;
      this.expanded.delete(first);
    }
    for (const object of this.terminalBodies.keys()) if (!this.expanded.has(object)) this.terminalBodies.delete(object);
  }

  deferred(key: string): boolean {
    const entry = this.get(key);
    const object = entry && this.objectOf(entry);
    return !!object && !this.expanded.has(object.object);
  }

  private objectOf(entry: Entry): NonNullable<Window["pages"]>["objects"][number] | undefined {
    const window = this.pageWindow;
    return window?.pages?.objects.find((object) => {
      const event = window.events[object.at];
      if (event && (event.event === "finished" || event.event === "began" || event.event === "updated") && "item" in entry && "turn" in entry) {
        return event.turn === entry.turn && (event.item.id === entry.item || event.item.draft === entry.item);
      }
      return object.at === entry.at;
    });
  }

  get paged(): boolean { return this.pageWindow !== null; }

  follow(following: boolean): void {
    if (!this.pageWindow || this.follows === following) return;
    this.follows = following;
    void this.source?.follow?.(++this.followRevision, following).catch((error: unknown) => console.warn("the reader's position could not be sent", error));
  }

  async newest(): Promise<void> {
    if (this.pageWindow?.pages?.newer) await this.source?.part(this.generation, { edge: "latest" });
  }

  /** Facts a run's job told about a run an earlier part holds go to it as
   *  well: a job goes on after its turn. So does what one of the host's own
   *  agents did, to the part holding the call that started it: an agent
   *  goes on in the background for as long as it likes. */
  private route(event: SessionEvent): void {
    if (event.event !== "changed") return;
    const change = event.change;
    if (change.kind === "run_facts_updated") {
      const call = change.update.call_id;
      if (this.live.holdsCall(call)) return;
      for (const part of this.parts) if (part.fold.holdsCall(call)) part.fold.late(event);
      return;
    }
    const agent =
      change.kind === "agent_started" ? change.agent.id : change.kind === "agent_event" || change.kind === "agent_ended" ? change.agent_id : null;
    if (agent === null) return;
    const call = change.kind === "agent_started" ? change.agent.call : undefined;
    for (const part of this.parts) {
      if (part.fold === this.live) continue;
      if (part.fold.agents.get(agent) || (call && part.fold.holdsCall(call))) part.fold.late(event);
    }
  }

  /** What came before arrived: everything held moves along. */
  rebased(generation: number, offset: number, turnsAdded: number): void {
    this.generation = generation;
    for (const part of this.parts) {
      part.start += offset;
      part.end += offset;
      part.turnsBefore += turnsAdded;
    }
    this.structure += 1;
  }

  /** Reads into a gap from the side the reader comes from: from its end,
   *  next to the part after it, when the reader comes from below; from its
   *  start otherwise. */
  async fill(gap: Gap, side: "end" | "start"): Promise<void> {
    if (this.pageWindow) side = gap.first ? "end" : "start";
    const edge: Edge = side === "end" ? { edge: "before", at: gap.end, floor: gap.start } : { edge: "after", at: gap.start, ceiling: gap.end };
    await this.read(gap, edge);
  }

  /** Reads the turns around event `at` into the gap that holds it; true
   *  once they are held. */
  async reach(at: number): Promise<boolean> {
    const gap = this.gaps().find((one) => one.start <= at && at < one.end);
    if (!gap) return this.parts.some((part) => part.start <= at && at < part.end);
    return this.read(gap, { edge: "around", at, floor: gap.start, ceiling: gap.end });
  }

  positionOf(key: string): TimelinePosition | null {
    const block = this.blocks().find((block) => block.key === key);
    const entry = block?.kind === "activity-group" ? block.members[0]?.key : block?.key ?? key;
    const place = entry ? this.live.placeOf(entry) : null;
    const at = place ? this.pageWindow?.pages?.keys?.indexOf(place) : -1;
    return at !== undefined && at >= 0 ? this.pageWindow?.pages?.places?.[at] ?? null : null;
  }

  entriesAtPosition(place: TimelinePosition): Entry[] {
    const at = this.pageWindow?.pages?.places?.findIndex((current) => current?.item === place.item) ?? -1;
    const key = this.pageWindow?.pages?.keys?.[at];
    return key ? this.live.entries.filter((entry) => this.live.placeOf(entry.key) === key) : [];
  }

  async expandPosition(place: TimelinePosition): Promise<void> {
    const at = this.pageWindow?.pages?.places?.findIndex((current) => current?.item === place.item) ?? -1;
    const object = this.pageWindow?.pages?.objects.find((object) => object.at === at);
    if (object && object.bytes <= 8 * 1024 * 1024) await this.readFull(this.entriesAtPosition(place)[0]?.key ?? "");
  }

  async reachPosition(place: TimelinePosition): Promise<boolean> {
    if (!this.source || !this.pageWindow) return false;
    await this.source.part(this.generation, { edge: "position", place });
    return this.pageWindow.pages?.places?.some((current) => current?.item === place.item) === true;
  }

  /** Reads the turns where `turn` first appears, wherever they are; true
   *  once they are held. */
  async reachTurn(turn: string): Promise<boolean> {
    if (this.foldOf(turn)) return true;
    const source = this.source;
    if (!source) return false;
    if (this.pageWindow) {
      await source.part(this.generation, { edge: "turn", turn });
      return this.foldOf(turn) !== null;
    }
    const at = await source.locate(this.generation, turn).catch(() => null);
    return at !== null && this.reach(at);
  }

  private async read(gap: Gap, edge: Edge): Promise<boolean> {
    const source = this.source;
    if (!source || this.reading.has(gap.key)) return false;
    this.reading.add(gap.key);
    this.structure += 1;
    this.changed();
    const generation = this.generation;
    try {
      const part = await source.part(generation, edge);
      if (!part || part.generation !== this.generation || generation !== this.generation) return false;
      // Another reading may have filled some of it meanwhile.
      if (this.parts.some((held) => held.start < part.end && part.start < held.end)) return false;
      const fold = new Fold(`p${++this.folds}`);
      fold.thoughtWords = this.thoughts;
      fold.history(part.events);
      for (const event of part.late) fold.late(event);
      const at = this.parts.findIndex((held) => held.start >= part.end);
      this.parts.splice(at < 0 ? this.parts.length - 1 : at, 0, { start: part.start, end: part.end, turnsBefore: part.turns_before, turns: part.turns, fold });
      return true;
    } catch (error) {
      this.readingFailure = String(error);
      return false;
    } finally {
      this.reading.delete(gap.key);
      this.structure += 1;
      this.changed();
    }
  }

  /** Lets go of an earlier part, which becomes a gap again. */
  release(part: Held): void {
    const at = this.parts.indexOf(part);
    if (at < 0 || at === this.parts.length - 1) return;
    this.parts.splice(at, 1);
    this.structure += 1;
  }

  /** Lets go of what the latest part holds before its latest `events`
   *  events, where the shell can cut there. */
  async trim(events: number): Promise<void> {
    const source = this.source;
    if (!source) return;
    const generation = this.generation;
    const cut = await source.cut(generation, events).catch(() => null);
    const tail = this.tail;
    if (!cut || generation !== this.generation || cut.at <= tail.start || cut.at > tail.end) return;
    tail.fold.forget(cut.at - tail.start);
    tail.start = cut.at;
    tail.turnsBefore = cut.turns_before;
    // An earlier part that ran up to the latest one stays where it is: the
    // turns between are a gap now.
    this.structure += 1;
  }

  /** Every entry of the conversation in order, a part at a time: what is
   *  held as it is, the rest read from the shell and let go once passed;
   *  ends early if the conversation is read again meanwhile. */
  async *everything(): AsyncGenerator<ExportReading> {
    const source = this.source;
    const generation = this.generation;
    if (this.pageWindow) {
      if (!source?.scan) throw new Error("this source cannot export the complete conversation");
      let cursor: ScanCursor | null = null;
      do {
        const scanned = await source.scan(cursor);
        if (scanned.restart) yield { restart: true };
        const fold = new Fold("export");
        fold.thoughtWords = this.thoughts;
        fold.history(scanned.events);
        let start = 0;
        for (const preview of scanned.truncated ?? []) {
          const after = fold.entries.findIndex((entry) => entry.at > preview.at);
          const end = after < 0 ? fold.entries.length : after;
          yield fold.entries.slice(start, end);
          yield { previewBytes: preview.bytes };
          start = end;
        }
        yield fold.entries.slice(start);
        cursor = scanned.next;
      } while (cursor);
      return;
    }
    let at = 0;
    for (const part of [...this.parts]) {
      while (source && at < part.start) {
        const read = await source.part(generation, { edge: "after", at, ceiling: part.start });
        if (!read || read.end <= at) return;
        const fold = new Fold("x");
        fold.thoughtWords = this.thoughts;
        fold.history(read.events);
        for (const event of read.late) fold.late(event);
        yield fold.entries;
        at = read.end;
      }
      yield part.fold.entries;
      at = part.end;
    }
  }

  /** The gaps between and before the parts held. */
  gaps(): Gap[] {
    return this.blocks().filter((block): block is Gap => block.kind === "gap");
  }

  /** What is laid out: each part's blocks, gaps where turns are not held.
   *  A fork's record begins with what it carried over: from the part that
   *  holds the point it was made at, the card there stands for all before. */
  blocks(): Block[] {
    const version = `${this.structure}|${this.parts.map((part) => part.fold.clock).join(",")}`;
    if (this.cache?.version === version) return this.cache.blocks;
    const blocks: Block[] = [];
    if (this.pageWindow?.pages) {
      const pages = this.pageWindow.pages;
      const end = this.tail.end;
      const before = `timeline:${this.generation}:before`;
      const after = `timeline:${this.generation}:after`;
      if (pages.older) blocks.push({ kind: "gap", key: before, stamp: this.reading.has(before) ? 1 : 0, start: 0, end: 0, turns: 0, first: true, loading: this.reading.has(before), paged: true });
      const current = this.live.blocks().map((block, at, all) => block.kind === "activity-group" ? {
        ...block, partialBefore: at === 0 && pages.partial_older === true,
        partialAfter: at === all.length - 1 && pages.partial_newer === true,
      } : block);
      const pending = pages.newer ? current.filter((block) => block.kind === "ask") : [];
      blocks.push(...current.filter((block) => !pages.newer || block.kind !== "ask"));
      if (pages.newer) blocks.push({ kind: "gap", key: after, stamp: this.reading.has(after) ? 1 : 0, start: end, end, turns: 0, first: false, loading: this.reading.has(after), later: true, paged: true });
      blocks.push(...pending);
      this.cache = { version, blocks };
      return blocks;
    }
    let end = 0;
    let turns = 0;
    const forkAt = this.parts.findIndex((part) => part.fold.holdsFork);
    for (const [index, part] of this.parts.entries()) {
      if (index < forkAt) {
        end = part.end;
        turns = part.turnsBefore + part.turns;
        continue;
      }
      if (part.start > end && index !== forkAt) {
        const key = `gap:${part.start}`;
        const loading = this.reading.has(key);
        const count = Math.max(0, part.turnsBefore - turns);
        blocks.push({ kind: "gap", key, stamp: count * 2 + (loading ? 1 : 0), start: end, end: part.start, turns: count, first: end === 0, loading });
      }
      for (const block of part.fold.blocks()) blocks.push(block);
      end = part.end;
      turns = part.turnsBefore + part.turns;
    }
    this.cache = { version, blocks };
    return blocks;
  }

  /** What the conversation's own view shows (`world-voice.md`, 两种看法):
   *  the user's words, each turn's answer, what waits on them, what it asked
   *  without stopping, and a turn that failed. A turn's answer is its last
   *  words once it has ended, whoever began it; a colleague's turn that
   *  spoke to the user through `message_user` shows what it said instead,
   *  and its last words too only when it worked on after saying it. */
  chatBlocks(): Block[] {
    const blocks = this.blocks();
    const version = this.cache!.version;
    if (this.chatCache?.version === version) return this.chatCache.blocks;
    const shown = blocks.filter((block) => {
      switch (block.kind) {
        case "say":
        case "gap":
        case "ask":
        case "question":
        case "command":
        case "forked":
        case "uncertain":
          return true;
        case "heard":
          return !block.notice && (!block.from || block.from.kind === "user");
        case "prose":
          return block.prose === "message" && (!this.spokeIn(block.turn) || this.answersAfterSaying(block.turn)) && this.turnEnded(block.turn) && this.lastMessageOf(block.turn) === block.key;
        case "ended":
          return block.end.how === "failed";
        default:
          return false;
      }
    });
    // A moment stands above the first bubble, and above any said a while
    // after the one before it, as a chat says when (user 2026-10-29).
    const times = new Map<string, number>();
    const before = new Map<string, Block | null>();
    let last: number | null = null;
    shown.forEach((block, index) => {
      before.set(block.key, shown[index - 1] ?? null);
      const time = block.kind === "heard" || block.kind === "say" ? (block.time ?? null) : null;
      if (time === null) return;
      if (last === null || time - last >= CHAT_GAP_MS) times.set(block.key, time);
      last = time;
    });
    this.chatTimes = times;
    this.chatBefore = before;
    this.chatCache = { version, blocks: shown };
    return shown;
  }

  /** The moment the chat view says above the bubble `key`, where it says
   *  one (`chatBlocks`). */
  chatTime(key: string): number | null {
    this.chatBlocks();
    return this.chatTimes.get(key) ?? null;
  }
  private chatTimes = new Map<string, number>();

  /** What the chat view shows before `key`: null for its first, undefined
   *  where it does not show `key` at all. */
  chatPrevious(key: string): Block | null | undefined {
    this.chatBlocks();
    return this.chatBefore.get(key);
  }
  private chatBefore = new Map<string, Block | null>();

  /** The key of the last thing the agent said to the user in `turn`. */
  lastSayOf(turn: string): string | null {
    const fold = this.foldOf(turn);
    if (!fold?.spokeIn(turn)) return null;
    for (let at = fold.entries.length - 1; at >= 0; at -= 1) {
      const entry = fold.entries[at]!;
      if (entry.kind === "tool" && entry.turn === turn && sayOf(entry)) return entry.key;
    }
    return null;
  }

  /** Whether the agent said something to the user in `turn` through
   *  `message_user`: the rest of that turn's words are its working notes. */
  spokeIn(turn: string): boolean {
    return this.foldOf(turn)?.spokeIn(turn) ?? false;
  }

  /** Whether `turn` went on working after the last thing it said to the
   *  user and ended in words of its own: those words are its answer too,
   *  said after it, as a model's that said it would look and then wrote
   *  what it found as its final words (`world-voice.md`, 两种看法). */
  answersAfterSaying(turn: string): boolean {
    const fold = this.foldOf(turn);
    if (!fold?.spokeIn(turn) || !this.turnEnded(turn)) return false;
    const mine = fold.entries.filter((entry) => "turn" in entry && entry.turn === turn);
    const after = mine.slice(mine.findLastIndex((entry) => entry.kind === "tool" && !!sayOf(entry)) + 1);
    return after.some((entry) => entry.kind === "tool") && after.some((entry) => entry.kind === "prose" && entry.prose === "message");
  }

  // What the page asks, as of one conversation.

  /** Every fold asked, the latest first, those read aside last. */
  private *folded(): Iterable<Fold> {
    for (let at = this.parts.length - 1; at >= 0; at -= 1) yield this.parts[at]!.fold;
    yield* this.aside;
  }

  /** An entry by its key, in whichever part holds it. */
  get(key: string): Entry | undefined {
    for (const fold of this.folded()) {
      const found = fold.get(key);
      if (found) return found;
    }
    return undefined;
  }

  /** The machine holding the file `path` named in the words of `key`, as
   *  what their turn did says (`acted-on.ts`); null for the conversation's
   *  own host, or where nothing says. */
  machineNamed(key: string, path: string): MachineName | null {
    const words = this.get(key);
    const turn = words && "turn" in words && typeof words.turn === "string" ? words.turn : null;
    const fold = turn === null ? null : this.foldOf(turn);
    return words && turn !== null && fold ? machineNamed(fold.turnEntries(turn), words, path) : null;
  }

  /** The part holding `turn`. */
  private foldOf(turn: string): Fold | null {
    for (const fold of this.folded()) if (fold.holds(turn)) return fold;
    return null;
  }

  attachmentName(path: string): string | null {
    for (const fold of this.folded()) {
      const name = fold.attachmentName(path);
      if (name) return name;
    }
    return null;
  }

  thinkingTokens(run: readonly Entry[]): number | null {
    const first = run[0];
    if (!first) return null;
    for (const fold of this.folded()) if (fold.get(first.key) === first) return fold.thinkingTokens(run);
    return this.live.thinkingTokens(run);
  }

  // Finding in what the page does not hold.

  /** Reads `start..end` aside, as finding does: folded, asked as the parts
   *  held are, never laid out; let go with what it answers. */
  async readAside(start: number, end: number): Promise<{ fold: Fold; start: number; end: number; release(): void } | null> {
    const source = this.source;
    if (!source) return null;
    const read = await source.part(this.generation, { edge: "after", at: start, ceiling: end }).catch(() => null);
    if (!read || read.generation !== this.generation) return null;
    const fold = new Fold(`f${++this.folds}`);
    fold.thoughtWords = this.thoughts;
    fold.history(read.events);
    for (const event of read.late) fold.late(event);
    this.aside.push(fold);
    return {
      fold,
      start: read.start,
      end: read.end,
      release: () => {
        this.aside = this.aside.filter((one) => one !== fold);
      },
    };
  }

  /** Hands the shell the displayed text of `start..end`. */
  async indexFar(start: number, end: number, docs: FindDoc[]): Promise<boolean> {
    return (await this.source?.index(this.generation, start, end, docs).catch(() => false)) ?? false;
  }

  /** Counts words in what the page does not hold, as the shell has the
   *  text of it; null where nothing is read from a shell. */
  async findFar(query: string): Promise<Found | null> {
    const source = this.source;
    if (!source) return null;
    const found = await source.find(this.generation, query, this.bounds()).catch(() => null);
    return found && found.generation === this.generation ? found : null;
  }

  /** The agent a call started (`Fold.agentOf`), in the part holding it. */
  agentOf(entry: ToolEntry): AgentRun | null {
    for (const part of this.parts) {
      if (!part.fold.holdsCall(entry.item)) continue;
      const found = part.fold.agentOf(entry);
      if (found) return found;
    }
    return this.live.agentOf(entry);
  }

  /** The agents a call names (`Fold.agentsNamed`). */
  agentsNamed(entry: ToolEntry): AgentRun[] {
    for (const part of this.parts) if (part.fold.holdsCall(entry.item)) return part.fold.agentsNamed(entry);
    return [];
  }

  /** One of the host's own agents, by its id, in whichever part knows it. */
  agent(id: string): AgentRun | null {
    for (let at = this.parts.length - 1; at >= 0; at -= 1) {
      const found = this.parts[at]!.fold.agents.get(id);
      if (found) return found;
    }
    return null;
  }

  /** The host's own agents of every part held, each once, in the order
   *  they began. */
  agents(): AgentRun[] {
    const found = new Map<string, AgentRun>();
    for (const part of this.parts) for (const run of part.fold.agents.all()) if (!found.has(run.id)) found.set(run.id, run);
    return [...found.values()].sort((a, b) => (a.began ?? 0) - (b.began ?? 0));
  }

  /** The entry of a call, in whichever part holds it. */
  call(call: string): ToolEntry | null {
    for (const part of this.parts) {
      const found = part.fold.call(call);
      if (found) return found;
    }
    return null;
  }

  /** What was seen live broke off: no agent seen so far is whole. */
  broke(): void {
    for (const part of this.parts) part.fold.agents.broke();
  }

  lastMessageOf(turn: string): string | null {
    return this.foldOf(turn)?.lastMessageOf(turn) ?? null;
  }

  /** Where a fork from `turn`, or from where the conversation is now, would
   *  begin: the user's words that asked, the answer to them, and the key of
   *  the block the answer is drawn in. */
  excerpt(turn: string | null): { said: string | null; answer: string | null; key: string | null } {
    const blocks = this.blocks();
    let answer: Extract<Block, { kind: "prose" | "say" }> | null = null;
    for (const block of blocks) {
      if (!((block.kind === "prose" && block.prose === "message") || block.kind === "say")) continue;
      if (turn === null || block.turn === turn) answer = block;
    }
    const at = answer?.turn ?? turn;
    const said = blocks.find((block): block is Extract<Block, { kind: "heard" }> => block.kind === "heard" && !block.notice && (!block.from || block.from.kind === "user") && block.turn === at) ?? null;
    return { said: said?.text ?? null, answer: answer?.text ?? null, key: answer?.key ?? said?.key ?? null };
  }

  turnEnded(turn: string): boolean {
    return this.foldOf(turn)?.turnEnded(turn) ?? false;
  }

  /** The turn whose actions stay shown: the last that ended. */
  latestEnded(): string | null {
    for (let at = this.parts.length - 1; at >= 0; at -= 1) {
      const turn = this.parts[at]!.fold.latestEnded();
      if (turn) return turn;
    }
    return null;
  }

  /** The user's own words a turn began with, where they are held. */
  turnWords(turn: string): Heard | null {
    const fold = this.foldOf(turn);
    return (fold?.entries.find((entry): entry is Heard => entry.kind === "heard" && entry.turn === turn && (!entry.from || entry.from.kind === "user")) ?? null);
  }

  /** All a turn said, as it was written. */
  turnAnswer(turn: string): string {
    const fold = this.foldOf(turn);
    // A turn that said something to the user answered with that, and with
    // its final words where it went on working after the last of it.
    if (fold?.spokeIn(turn)) {
      const said = fold.entries
        .flatMap((entry) => (entry.kind === "tool" && entry.turn === turn ? [sayOf(entry)] : []))
        .flatMap((say) => (say ? [say.text] : []));
      const final = this.lastMessageOf(turn);
      const words = this.answersAfterSaying(turn) ? fold.entries.find((entry): entry is ProseEntry => entry.kind === "prose" && entry.key === final) : undefined;
      if (words) said.push(words.text);
      return said.join("\n\n");
    }
    return (fold?.entries ?? [])
      .filter((entry): entry is ProseEntry => entry.kind === "prose" && entry.prose === "message" && entry.turn === turn)
      .map((entry) => entry.text)
      .join("\n\n");
  }

  waiting(): boolean {
    return this.live.waiting();
  }
  busy(): boolean {
    return this.live.busy();
  }
  /** No turn runs, the World says: nothing shown is under way any more. */
  noTurnRuns(): void {
    this.live.noTurnRuns();
  }
  /** The user asked what runs to stop: it shows stopped at once. */
  halt(): void {
    this.live.halt();
  }
  /** The stop did not reach the agent: it shows going on again. */
  unhalt(): void {
    this.live.unhalt();
  }
  halting(): boolean {
    return this.live.halting();
  }
  turning(): boolean {
    return this.live.turning();
  }
  turnBegan(): number | null {
    return this.live.turnBegan();
  }
  moving(): boolean {
    return this.live.moving();
  }
  ended(): number {
    return this.live.ended();
  }
  lastEnd(): TurnEnd | null {
    return this.live.lastEnd();
  }
  /** Whether the agent has shown anything of its own yet: anything before
   *  the latest part is. */
  spoke(): boolean {
    return this.live.spoke() || this.parts.length > 1 || this.tail.start > 0;
  }
  hasTurns(): boolean {
    return this.tail.start > 0 || this.parts.some((part) => part.fold.hasTurns());
  }
  takeControlsChanged(): boolean {
    return this.live.takeControlsChanged();
  }
  controlState(id: string): PendingControl | undefined {
    return this.live.controlState(id);
  }
  takeOffersChanged(): boolean {
    return this.live.takeOffersChanged();
  }

  // What the page does to it, live.

  sending(text: string, here: MachineName | null, attached: ShownAttachment[] = []): string {
    return this.live.sending(text, here, attached);
  }
  sent(key: string, state: SendState): void {
    this.live.sent(key, state);
  }
  named(key: string, delivery: string | null): void {
    this.live.named(key, delivery);
  }
  /** What its machine holds of it now (`Fold.now`). */
  now(busy: boolean, turn: string | null, kept: KeptWords[]): void {
    this.live.now(busy, turn, kept);
  }
  unsend(key: string): void {
    this.live.unsend(key);
  }
  /** `Fold.settles`, `Fold.settler`. */
  settles(key: string, settler: Settler | null): void {
    this.live.settles(key, settler);
  }
  settler(key: string): Settler | undefined {
    return this.live.settler(key);
  }
  /** `Fold.heardMark`, `Fold.heardAlready`. */
  heardMark(): number {
    return this.live.heardMark();
  }
  heardAlready(text: string, mark: number | null): boolean {
    return this.live.heardAlready(text, mark);
  }
  failedToStart(why: string): void {
    this.live.failedToStart(why);
  }
  settle(ask: Extract<Entry, { kind: "ask" }>, state: AskState): void {
    (this.parts.find((part) => part.fold.get(ask.key) === ask)?.fold ?? this.live).settle(ask, state);
  }

  // Read whole, where nothing else holds it (a specimen, a conversation
  // being begun).

  /** What came before, replacing what is shown. */
  history(events: SessionEvent[]): void {
    this.live.history(events);
    this.parts = [{ start: 0, end: events.length, turnsBefore: 0, turns: 0, fold: this.live }];
    this.structure += 1;
  }

  extend(events: SessionEvent[]): void {
    this.live.extend(events);
    this.tail.end += events.length;
  }

  event(event: SessionEvent): void {
    this.live.event(event);
    this.pageWindow?.events.push(event);
    this.tail.end += 1;
  }
}
