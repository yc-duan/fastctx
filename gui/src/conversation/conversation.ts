import { sameEvent } from "./event-identity";
// A conversation as its page holds it (`world-gui.md`, 对话: 很长的对话). The
// page holds its latest part, kept current, and whatever earlier parts its
// reader came to, each folded on its own (`model.ts`). What is not held
// stands as a gap, before everything held or between two parts, as tall as
// it would be drawn, read when the reader comes to it and let go again when
// the reader is far away. So however long a conversation grows, the page
// holds and draws a few thousand events of it at most, and its reader goes
// anywhere in it as in one page.
//
// A conversation is shown from its timeline (`replica.ts`): parts read from
// the copy this computer holds, each event at its place, changed in place
// as the timeline's records say. One on a machine of an earlier release is
// watched whole instead, its events numbered from its first, the shell
// keeping all of them (`src-tauri/src/transcript.rs`).
//
// Everything a page asks of a conversation is asked here, as of one: what
// it lays out, an entry by its key, what a turn said, whether it is busy.
// Each question goes to the part that holds its answer; what is live (words
// being sent, requests waiting, whether it works now) is in the latest.

import { askKey, type AskState } from "./ask-state";
import { inOrder, type ModelRequest } from "./requests";
import type { SessionEvent, TurnEnd } from "../world/events";
import type { MachineName } from "../world/events";
import { Fold, sayOf, type Block, type Entry, type Gap, type KeptWords, type SendState, type Settler, type ShownAttachment, type StartShown } from "./model";
import type { AgentRun } from "./agents";
import { machineNamed } from "./acted-on";
import type { PendingControl } from "../world/control";
import { Measured, before, begins, insert, member, ordinal, placeAt, removeAt, spanOf, type Place, type ReadFrom, type ReplicaPart, type ReplicaRecord, type Span } from "./replica";

export type TimelinePosition = Place;

/** The part a page opens on, in place of all it held, of a conversation
 *  watched whole. */
export interface Window {
  generation: number;
  /** Where its events begin, among all the conversation's. */
  start: number;
  turns_before: number;
  events: SessionEvent[];
  /** What its turns read of what came before them. */
  carry: SessionEvent[];
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
}

export type Edge =
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

export interface ExpandedEvent { object: string; event: SessionEvent }
export type ExportReading = readonly Entry[] | { previewBytes: number } | { restart: true };

/** Where the parts a page does not hold are read from. */
export interface PartSource {
  identity?(): string;
  search?(request: import("./timeline-find").SearchRequest): Promise<import("./timeline-find").SearchReply>;
  /** A part of a conversation watched whole. */
  part(generation: number, edge: Edge): Promise<PartRead | null>;
  /** A part of a conversation shown from its timeline; none when the
   *  timeline was made anew since. */
  page?(generation: number, from: ReadFrom): Promise<ReplicaPart | null>;
  /** An event kept whole as an object. */
  object?(object: string, bytes: number): Promise<ExpandedEvent>;
  /** The files it changed as the machine it lives on keeps them. */
  files?(after: string): Promise<import("../world/bridge").FileChangesTold>;
  /** Where a turn first appears. */
  locate(generation: number, turn: string): Promise<number | null>;
  cut(generation: number, events: number): Promise<Cut | null>;
  index(generation: number, start: number, end: number, docs: FindDoc[]): Promise<boolean>;
  find(generation: number, query: string, held: [number, number][]): Promise<Found | null>;
}

/** A part held, folded. Of a conversation watched whole it is the events
 *  `start..end`, counted from its first; of one shown from its timeline,
 *  the stretch of it between those places, counted as `replica.ts` counts
 *  them. */
export interface Held {
  start: number;
  end: number;
  turnsBefore: number;
  turns: number;
  fold: Fold;
}

/** Where the reader stands in a gap the view came into: how far down it,
 *  how much of it is above the view, and how tall it is drawn. */
export interface Into {
  fraction: number;
  above: number;
  height: number;
}

type Heard = Extract<Entry, { kind: "heard" }>;
type ProseEntry = Extract<Entry, { kind: "prose" }>;
type ToolEntry = Extract<Entry, { kind: "tool" }>;

/** The most bytes of events opened whole a page keeps. */
const EXPANDED_BYTES = 8 * 1024 * 1024;

/** A message the timeline just gained is shown as it would be written, a
 *  little at a time: how long that takes at most and at least, how often a
 *  little more is shown, how lately the message must have been written,
 *  how short it may be, and how many records may wait behind it before it
 *  is shown whole at once. */
const TYPED_MOST_MS = 2400;
const TYPED_LEAST_MS = 400;
const TYPED_EVERY_MS = 32;
const TYPED_WITHIN_MS = 120_000;
const TYPED_LEAST_CHARS = 24;
const TYPED_WAITING_MOST = 64;

/** How long after one bubble the next has its moment said above it. */
const CHAT_GAP_MS = 5 * 60_000;
export class Conversation {
  searchVersion = 0;
  private searchSource = 0;
  get searchIdentity(): string { return this.source?.identity?.() ?? String(this.searchSource); }
  /** What a read is a read of: its source and the generation of its
   *  timeline, which a read finished under another no longer speaks to. */
  get readEpoch(): string { return `${this.searchIdentity}#${this.generation}`; }
  search(request: import("./timeline-find").SearchRequest): Promise<import("./timeline-find").SearchReply> {
    if (!this.source?.search) return Promise.reject(new Error("this source cannot search the complete history"));
    return this.source.search(request);
  }
  /** The files it changed as the machine it lives on keeps them, after a
   *  place an earlier reading gave; none where its source keeps none. */
  files(after: string): Promise<import("../world/bridge").FileChangesTold> | null {
    return this.source?.files?.(after) ?? null;
  }
  /** The parts held, in order; the last is the latest, and live. */
  private parts: Held[];
  private generation = 0;
  /** What each part read from the timeline holds besides its fold; none
   *  of a conversation watched whole. */
  private spans = new Map<Held, Span>();
  /** Whether it is shown from its timeline. */
  private replica = false;
  /** Whether its timeline is still being made from the history. */
  private making = false;
  /** The requests that wait for the user, as its machine tells them. */
  private asks: SessionEvent[] = [];
  /** How tall stretches no longer held were. */
  private measured = new Measured();
  /** The records that came while a page was being read, for each reading
   *  under way: a page read before them is set right by them once it comes. */
  private meanwhile = new Set<ReplicaRecord[]>();
  /** What its machine last told it holds now (`now`), told again to a
   *  latest part folded anew, which has only its records to go by. */
  private nowTold: { busy: boolean; turn: string | null; kept: KeptWords[]; halted: string | null } | null = null;
  /** Events opened whole, by the object each is kept as. */
  private expanded = new Map<string, SessionEvent>();
  /** The message being shown as it would be written, and the records that
   *  came after it and wait for it. */
  private typing: { shown: import("./replica").Shown; timer: ReturnType<typeof setTimeout> } | null = null;
  private behind: ReplicaRecord[] = [];
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

  /** Whether its timeline is still being made from the history. */
  get preparing(): boolean { return this.replica && this.making; }

  /** Whether parts of it are read from a shell. */
  get windowed(): boolean {
    return this.source !== null;
  }

  /** Whether it is shown from its timeline, each event at a place. */
  get paged(): boolean { return this.replica; }

  // Watched whole: reading it from the shell.

  /** The part the page opens on, in place of all it held. */
  open(window: Window, source: PartSource | null): void {
    this.searchVersion += 1;
    if (source !== this.source) this.searchSource += 1;
    this.generation = window.generation;
    this.replica = false;
    this.making = false;
    this.spans.clear();
    this.measured.clear();
    this.expanded.clear();
    this.source = source;
    const tail = this.live;
    tail.history([...window.events], window.carry);
    this.parts = [{ start: window.start, end: window.start + window.events.length, turnsBefore: window.turns_before, turns: 0, fold: tail }];
    this.reading.clear();
    this.structure += 1;
  }

  /** Events kept after all that was given; false when they do not follow
   *  what is held, and it must be read again. */
  extended(start: number, events: SessionEvent[]): boolean {
    if (this.replica || start !== this.tail.end) return false;
    this.live.extend(events);
    this.tail.end += events.length;
    return true;
  }

  /** One thing as it happened, the `at`th event; false when it does not
   *  follow what is held. */
  happened(at: number, event: SessionEvent): boolean {
    if (this.replica || at !== this.tail.end) return false;
    this.route(event);
    this.live.event(event);
    this.tail.end += 1;
    return true;
  }

  // Shown from its timeline.

  /** The latest part of its timeline, in place of all the page held:
   *  opened, or its timeline made anew. */
  openReplica(part: ReplicaPart, source: PartSource | null): void {
    this.typed(false);
    this.searchVersion += 1;
    if (source !== this.source) this.searchSource += 1;
    this.generation = part.generation;
    this.replica = true;
    this.making = part.preparing;
    this.source = source;
    this.measured.clear();
    this.expanded.clear();
    const span = spanOf(part);
    span.last = true;
    const tail = this.live;
    tail.history([...span.events], [], span.keys, false, this.asks);
    const held = this.placed({ start: 0, end: 0, turnsBefore: 0, turns: 0, fold: tail }, span);
    this.spans = new Map([[held, span]]);
    this.parts = [held];
    this.reading.clear();
    this.structure += 1;
  }

  /** Whether its timeline is still being made, as its machine says now. */
  prepares(making: boolean): void {
    if (this.making === making) return;
    this.making = making;
    this.structure += 1;
  }

  /** Where `part` stands, by the first and last of what `span` holds. */
  private placed(part: Held, span: Span): Held {
    const first = span.places[0];
    const last = span.places[span.places.length - 1];
    part.start = first ? ordinal(first) : span.older ? ordinal(span.older) : 0;
    part.end = last ? ordinal(last) + 1 : part.start;
    return part;
  }

  /** The events of `span` as the page shows them: one opened whole in the
   *  place of what it was cut short to. */
  private shown(span: Span): SessionEvent[] {
    if (this.expanded.size === 0 || span.deferred.size === 0) return [...span.events];
    return span.events.map((event, at) => {
      const deferred = span.deferred.get(span.keys[at]!);
      return (deferred && this.expanded.get(deferred.object)) ?? event;
    });
  }

  /** The part that holds what stands at `place`, or takes what comes to
   *  stand there: the one it lies within; the one before it when the next
   *  begins where that one ends; the latest when it is past everything; the
   *  first when the timeline begins with it. None when it lies in a gap. */
  private partAt(place: Place): Held | null {
    for (const [index, part] of this.parts.entries()) {
      const span = this.spans.get(part);
      if (!span) continue;
      const first = span.places[0];
      const last = span.places[span.places.length - 1];
      if (!first || !last) {
        if (span.last) return part;
        continue;
      }
      if (before(place, first)) {
        // Before it: its own only where nothing of the timeline is before it.
        if (index === 0 && span.older === null) return part;
        const earlier = this.parts[index - 1];
        if (earlier && this.spans.get(earlier)?.joined) return earlier;
        return null;
      }
      if (!before(last, place) || span.last) return part;
    }
    return null;
  }

  /** Records its timeline gained, in the order they were written: each
   *  event stands as told from now on, where it is told to stand, and one
   *  gone is gone. What lies in a gap is read when the reader comes to it. */
  records(generation: number, records: readonly ReplicaRecord[]): void {
    if (!this.replica || generation !== this.generation) return;
    // Behind a message being written out, they wait their turn; too many
    // of them, and it is shown whole at once.
    if (this.typing) {
      this.behind.push(...records);
      if (this.behind.length > TYPED_WAITING_MOST) this.typed(true);
      return;
    }
    // A message just written, come by itself or with little else, is shown
    // as it would be written: what came before it is taken first, what came
    // after it waits.
    const at = records.length <= 8 ? records.findIndex((record) => this.writes(record)) : -1;
    if (at < 0) {
      this.take(records);
      return;
    }
    this.take(records.slice(0, at));
    this.behind.push(...records.slice(at + 1));
    this.write((records[at] as Extract<ReplicaRecord, { record: "is" }>).shown);
  }

  /** Whether `record` is a message the timeline just gained at its end,
   *  written lately: one to show as it would be written. */
  private writes(record: ReplicaRecord): boolean {
    if (record.record !== "is" || typeof document === "undefined" || document.visibilityState !== "visible") return false;
    const event = record.shown.event;
    if (event.event !== "finished" || event.item.kind !== "message" || event.item.text.length < TYPED_LEAST_CHARS) return false;
    if (Math.abs(Date.now() - Date.parse(event.at)) > TYPED_WITHIN_MS) return false;
    const span = this.spans.get(this.tail);
    if (!span || this.parts.some((part) => this.spans.get(part)?.keys.includes(record.shown.key))) return false;
    const last = span.places[span.places.length - 1];
    return !last || before(last, record.shown.place);
  }

  /** Shows the message `shown` as it would be written: begun empty, a
   *  little more of its words every moment, then as the timeline holds it. */
  private write(shown: import("./replica").Shown): void {
    const event = shown.event;
    if (event.event !== "finished" || event.item.kind !== "message") return;
    const text = event.item.text;
    const fold = this.tail.fold;
    fold.event({ ...event, event: "began", item: { ...event.item, text: "" } }, shown.key);
    const total = Math.min(TYPED_MOST_MS, Math.max(TYPED_LEAST_MS, text.length * 5));
    const each = Math.max(2, Math.ceil(text.length / (total / TYPED_EVERY_MS)));
    let at = 0;
    const more = () => {
      if (this.typing?.shown !== shown) return;
      let end = Math.min(text.length, at + each);
      // Never between the two halves of one character.
      const code = text.charCodeAt(end - 1);
      if (end < text.length && code >= 0xd800 && code <= 0xdbff) end += 1;
      this.tail.fold.event({ at: event.at, event: "grew", turn: event.turn, item: event.item.id, part: "text", text: text.slice(at, end) }, shown.key);
      at = end;
      if (at >= text.length) this.typed(true);
      else {
        this.typing.timer = setTimeout(more, TYPED_EVERY_MS);
        this.changed();
      }
    };
    this.typing = { shown, timer: setTimeout(more, TYPED_EVERY_MS) };
  }

  /** The message being written out stands whole, as the timeline holds it,
   *  and what waited behind it is taken; `drawn` when the page is to draw
   *  again for it. */
  private typed(drawn: boolean): void {
    const typing = this.typing;
    if (!typing) return;
    clearTimeout(typing.timer);
    this.typing = null;
    const behind = this.behind;
    this.behind = [];
    this.take([{ record: "is", shown: typing.shown }, ...behind]);
    if (drawn) this.changed();
  }

  /** Takes `records` in as they stand; `again` when they were taken before
   *  and are taken once more over a page read meanwhile. */
  private take(records: readonly ReplicaRecord[], again = false): void {
    if (records.length === 0) return;
    if (!again) for (const since of this.meanwhile) since.push(...records);
    this.searchVersion += 1;
    const touched = new Map<Held, { events: SessionEvent[]; keys: string[] }>();
    const touch = (part: Held, span: Span) => {
      if (!touched.has(part)) touched.set(part, { events: this.shown(span), keys: [...span.keys] });
    };
    for (const record of records) {
      const key = record.record === "is" ? record.shown.key : record.key;
      // The part that held it: where it stands on as it is told anew, should
      // the place it is told at be no part's by itself, as the first of a
      // part with a gap before it is not.
      let home: Held | null = null;
      for (const part of this.parts) {
        const span = this.spans.get(part);
        const at = span ? span.keys.indexOf(key) : -1;
        if (!span || at < 0) continue;
        touch(part, span);
        removeAt(span, at);
        home ??= part;
      }
      if (record.record !== "is") continue;
      const part = this.partAt(record.shown.place) ?? home;
      const span = part && this.spans.get(part);
      if (!part || !span) continue;
      // What a job or an agent told of a run an earlier part holds.
      this.route(record.shown.event);
      const last = span.places[span.places.length - 1];
      // Added at the end of the latest part, nothing else of it changed: it
      // is read as it comes, the part's other entries left as they are.
      if (part === this.tail && !touched.has(part) && (!last || before(last, record.shown.place))) {
        insert(span, record.shown);
        part.fold.event(record.shown.event, record.shown.key);
        this.placed(part, span);
        continue;
      }
      touch(part, span);
      insert(span, record.shown);
    }
    for (const [part, previous] of touched) this.refold(part, previous);
    if (touched.size) {
      this.parts = [...this.parts];
      this.structure += 1;
    }
  }

  /** Folds `part` again after its events changed, what did not change
   *  kept as it was drawn. */
  private refold(part: Held, previous: { events: readonly SessionEvent[]; keys: readonly string[] }): void {
    const span = this.spans.get(part);
    if (!span) return;
    part.fold = part.fold.revised(previous.events, previous.keys, this.shown(span), span.keys, part === this.tail ? this.asks : []);
    this.placed(part, span);
    this.relate(part);
    // Folded anew from its records alone, the latest part is told again
    // what its machine holds of it now.
    const now = this.nowTold;
    if (part === this.tail && now) part.fold.now(now.busy, now.turn, now.kept, now.halted);
  }

  /** Tells `part` what the parts after it hold of its runs and agents: a
   *  job goes on after its turn, and an agent for as long as it likes, so
   *  what became of them is written later than they are. */
  private relate(part: Held): void {
    const at = this.parts.indexOf(part);
    if (at < 0) return;
    for (const later of this.parts.slice(at + 1)) {
      for (const event of this.spans.get(later)?.events ?? []) {
        if (event.event !== "changed") continue;
        const change = event.change;
        if (change.kind === "run_facts_updated") {
          if (part.fold.holdsCall(change.update.call_id)) part.fold.late(event);
          continue;
        }
        const agent =
          change.kind === "agent_started" ? change.agent.id : change.kind === "agent_event" || change.kind === "agent_ended" ? change.agent_id : null;
        if (agent === null) continue;
        const call = change.kind === "agent_started" ? change.agent.call : undefined;
        if (part.fold.agents.get(agent) || (call && part.fold.holdsCall(call))) part.fold.late(event);
      }
    }
  }

  /** The requests that wait for the user now, all of them, as its machine
   *  tells them when the page begins to hear it. */
  waits(events: readonly SessionEvent[]): void {
    this.typed(false);
    this.asks = [...events];
    const span = this.spans.get(this.tail);
    if (!this.replica || !span) return;
    const events_now = this.shown(span);
    this.tail.fold = this.tail.fold.revised(events_now, span.keys, events_now, span.keys, this.asks);
    this.parts = [...this.parts];
    this.structure += 1;
  }

  /** A request that waits for the user, or the end of one, as it happens. */
  asked(event: SessionEvent): void {
    if (event.event !== "changed" || (event.change.kind !== "request" && event.change.kind !== "resolved")) return;
    const id = askKey(event.change.id);
    this.asks = this.asks.filter((held) => held.event !== "changed" || held.change.kind !== "request" || askKey(held.change.id) !== id);
    if (event.change.kind === "request") this.asks.push(event);
    this.live.livePending(event);
  }

  /** Where a held entry or block stands in the timeline. */
  private placeOfKey(key: string): { part: Held; span: Span; at: number } | null {
    for (const part of this.parts) {
      const span = this.spans.get(part);
      if (!span) continue;
      const placed = part.fold.placeOf(key);
      const at = placed === null ? -1 : span.keys.indexOf(placed);
      if (at >= 0) return { part, span, at };
    }
    return null;
  }

  /** Opens whole an event the page holds cut short, when its reader asks. */
  async readFull(key: string): Promise<void> {
    const found = this.placeOfKey(key);
    const source = this.source;
    const deferred = found && found.span.deferred.get(found.span.keys[found.at]!);
    if (!found || !deferred || !source?.object) return;
    const generation = this.generation;
    const expected = found.span.events[found.at];
    const expanded = await source.object(deferred.object, deferred.bytes);
    if (expanded.object !== deferred.object) throw new Error("the expanded object did not match the selected content");
    if (expected && !sameEvent(expected, expanded.event)) throw new Error("the expanded event did not match the selected entry");
    if (source !== this.source || generation !== this.generation) return;
    const span = this.spans.get(found.part);
    if (!span || !this.parts.includes(found.part)) return;
    const previous = { events: this.shown(span), keys: [...span.keys] };
    this.remember(deferred.object, expanded.event);
    this.refold(found.part, previous);
    this.parts = [...this.parts];
    this.structure += 1;
    this.changed();
  }

  /** Keeps an event opened whole, the earliest opened let go past the
   *  page's room for them. */
  private remember(object: string, event: SessionEvent): void {
    this.expanded.set(object, event);
    let bytes = [...this.expanded.values()].reduce((sum, held) => sum + JSON.stringify(held).length * 2, 0);
    while (bytes > EXPANDED_BYTES && this.expanded.size > 1) {
      const first = this.expanded.keys().next().value!;
      bytes -= JSON.stringify(this.expanded.get(first)).length * 2;
      this.expanded.delete(first);
    }
  }

  /** Whether the entry `key` is shown cut short, its whole not opened. */
  deferred(key: string): boolean {
    const found = this.placeOfKey(key);
    const deferred = found && found.span.deferred.get(found.span.keys[found.at]!);
    return !!deferred && !this.expanded.has(deferred.object);
  }

  /** `deferred`, asked of many entries at once: each entry's place is found
   *  once for them all, as `deferred` finds it (the first part that shows
   *  where the entry is first placed), where asking `deferred` of each
   *  would go through the page's events each time (codex-f9af46d4). */
  cutShort(): (key: string) => boolean {
    const any = this.parts.some((part) => [...(this.spans.get(part)?.deferred.values() ?? [])].some((deferred) => !this.expanded.has(deferred.object)));
    if (!any) return () => false;
    const decided = new Map<string, boolean>();
    for (const part of this.parts) {
      const span = this.spans.get(part);
      if (!span) continue;
      const shown = new Set(span.keys);
      const first = new Map<string, string>();
      for (const [place, entries] of part.fold.placements()) for (const entry of entries) if (!first.has(entry)) first.set(entry, place);
      for (const [entry, place] of first) {
        if (decided.has(entry) || !shown.has(place)) continue;
        const deferred = span.deferred.get(place);
        decided.set(entry, !!deferred && !this.expanded.has(deferred.object));
      }
    }
    return (key) => decided.get(key) ?? false;
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
    if (this.replica) return;
    this.generation = generation;
    for (const part of this.parts) {
      part.start += offset;
      part.end += offset;
      part.turnsBefore += turnsAdded;
    }
    this.structure += 1;
  }

  /** Reads into a gap the reader came to: from its end, next to the part
   *  after it, when the reader comes from below; from its start otherwise;
   *  and where the reader stands in it, when the view came into the middle
   *  of one it holds neither end of (`into`). */
  async fill(gap: Gap, side: "end" | "start", into?: Into): Promise<void> {
    if (this.replica) {
      await this.fillPlaces(gap, side, into);
      return;
    }
    const edge: Edge = side === "end" ? { edge: "before", at: gap.end, floor: gap.start } : { edge: "after", at: gap.start, ceiling: gap.end };
    await this.read(gap, edge);
  }

  /** Reads the turns around event `at` into the gap that holds it; true
   *  once they are held. */
  async reach(at: number): Promise<boolean> {
    const gap = this.gaps().find((one) => one.start <= at && at < one.end);
    if (!gap) return this.parts.some((part) => part.start <= at && at < part.end);
    if (this.replica) {
      const height = 0;
      await this.fillPlaces(gap, "start", { fraction: (at - gap.start) / Math.max(1e-9, gap.end - gap.start), above: -1, height });
      return this.parts.some((part) => part.start <= at && at < part.end);
    }
    return this.read(gap, { edge: "around", at, floor: gap.start, ceiling: gap.end });
  }

  /** Where the block or entry `key` stands in the timeline. */
  positionOf(key: string): TimelinePosition | null {
    const block = this.blocks().find((block) => block.key === key);
    const entry = block?.kind === "activity-group" ? block.members[0]?.key : block?.key ?? key;
    const found = entry ? this.placeOfKey(entry) : null;
    return found ? found.span.places[found.at] ?? null : null;
  }

  /** The entries the event at `place` is shown as. */
  entriesAtPosition(place: TimelinePosition): Entry[] {
    for (const part of this.parts) {
      const span = this.spans.get(part);
      if (!span || !span.keys.includes(place.item)) continue;
      return part.fold.entries.filter((entry) => part.fold.placeOf(entry.key) === place.item);
    }
    return [];
  }

  /** Opens whole the event at `place`, where it is held cut short. */
  async expandPosition(place: TimelinePosition): Promise<void> {
    const key = this.entriesAtPosition(place)[0]?.key;
    if (key && this.deferred(key)) await this.readFull(key);
  }

  /** Whether what the page holds has `place`. */
  holdsPosition(place: TimelinePosition): boolean {
    return this.parts.some((part) => this.spans.get(part)?.keys.includes(place.item));
  }

  /** Reads the part that holds `place`, wherever it is; true once held. */
  async reachPosition(place: TimelinePosition): Promise<boolean> {
    if (!this.replica) return false;
    const held = () => this.parts.some((part) => this.spans.get(part)?.keys.includes(place.item));
    if (held()) return true;
    const at = ordinal(place);
    const gap = this.gaps().find((one) => one.start <= at && at < one.end) ?? this.gaps().find((one) => at < one.end);
    if (!gap) return false;
    await this.fillPlaces(gap, "start", { fraction: 0, above: -1, height: 0 }, { order: place.order, item: "00" });
    return held();
  }

  /** Reads the turns where `turn` first appears, wherever they are; true
   *  once they are held. Of a conversation shown from its timeline, only
   *  what is held answers: a turn is found there by searching it. */
  async reachTurn(turn: string): Promise<boolean> {
    if (this.foldOf(turn)) return true;
    const source = this.source;
    if (!source || this.replica) return false;
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

  /** The parts either side of a gap of a conversation shown from its
   *  timeline: the one after it, and the one before it, none where nothing
   *  is held before it. */
  private around(gap: Gap): { earlier: Held | null; later: Held } | null {
    const at = this.parts.findIndex((part) => this.gapKey(this.parts[this.parts.indexOf(part) - 1] ?? null, part) === gap.key);
    if (at < 0) return null;
    return { earlier: this.parts[at - 1] ?? null, later: this.parts[at]! };
  }

  /** Reads a part of the timeline into `gap`. */
  private async fillPlaces(gap: Gap, side: "end" | "start", into?: Into, exact?: Place): Promise<void> {
    const source = this.source;
    const sides = this.around(gap);
    if (!source?.page || !sides || this.reading.has(gap.key)) return;
    const { earlier, later } = sides;
    const laterSpan = this.spans.get(later);
    const earlierSpan = earlier ? this.spans.get(earlier) : null;
    if (!laterSpan) return;
    // Where the page read begins: where the reader stands in the gap, when
    // that is away from both its ends; else at the end it came from.
    const span = gap.end - gap.start;
    const lands = exact ?? (into && into.fraction > 0.02 && into.fraction < 0.98 && span > 8 ? placeAt(gap.start + into.fraction * span) : null);
    const earlierLast = earlierSpan?.places[earlierSpan.places.length - 1] ?? null;
    let from: ReadFrom;
    let how: "end" | "start" | "into";
    if (lands) {
      from = { from: "after", place: lands };
      how = "into";
    } else if (side === "start" && earlierLast) {
      from = { from: "after", place: earlierLast };
      how = "start";
    } else {
      const edge = laterSpan.places[0] ?? laterSpan.older;
      if (!edge) return;
      from = { from: "before", place: edge };
      how = "end";
    }
    this.reading.add(gap.key);
    this.structure += 1;
    this.changed();
    const generation = this.generation;
    const since: ReplicaRecord[] = [];
    this.meanwhile.add(since);
    try {
      const read = await source.page(generation, from);
      if (!read || read.generation !== this.generation || generation !== this.generation) return;
      if (!this.parts.includes(later) || (earlier && !this.parts.includes(earlier))) return;
      // What another reading, or the timeline's records, brought meanwhile
      // is held already; what stands at or past the part after it is that
      // part's.
      const held = new Set(this.parts.flatMap((part) => this.spans.get(part)?.keys ?? []));
      const laterFirst = laterSpan.places[0] ?? null;
      const earlierEnd = earlierSpan?.places[earlierSpan.places.length - 1] ?? null;
      let reachedLater = false;
      let reachedEarlier = false;
      const events = read.events.filter((shown) => {
        if (held.has(shown.key)) {
          if (laterSpan.keys.includes(shown.key)) reachedLater = true;
          if (earlierSpan?.keys.includes(shown.key)) reachedEarlier = true;
          return false;
        }
        if (laterFirst && !before(shown.place, laterFirst)) { reachedLater = true; return false; }
        if (earlierEnd && !before(earlierEnd, shown.place)) { reachedEarlier = true; return false; }
        return true;
      });
      if (events.length === 0) {
        // Nothing lies there: the parts either side meet, or the timeline
        // begins at the part after it.
        if (how === "into") {
          await this.fromEnd(gap);
          return;
        }
        if (earlierSpan) earlierSpan.joined = true;
        else if (read.older === null || how === "end") laterSpan.older = null;
        return;
      }
      const part: ReplicaPart = { ...read, events };
      const fresh = spanOf(part);
      fresh.last = false;
      // It meets the part after it when it was read back from there, or read
      // on into it; the part before it, read on from there or back into it.
      fresh.joined = how === "end" || reachedLater;
      if (earlierSpan && (how === "start" || reachedEarlier)) earlierSpan.joined = true;
      if (!earlierSpan && how !== "into" && read.older === null) fresh.older = null;
      const fold = new Fold(`p${++this.folds}`);
      fold.thoughtWords = this.thoughts;
      fold.history([...fresh.events], [], fresh.keys, false, []);
      const made = this.placed({ start: 0, end: 0, turnsBefore: 0, turns: 0, fold }, fresh);
      this.spans.set(made, fresh);
      this.parts.splice(this.parts.indexOf(later), 0, made);
      this.parts = [...this.parts];
      // What the parts after it hold of its runs is told to it, and what it
      // holds of earlier parts' runs to them.
      this.relate(made);
      for (const event of fresh.events) this.route(event);
      // The page was read as the timeline stood when it was asked for:
      // records that came since stand over what it brought.
      if (since.length) this.take(since, true);
      // Read where the reader stood in the gap, it begins where the view
      // does: what is above it is as tall as it was drawn, what is below it
      // as tall as the rest of the gap was. Read from an end, only what it
      // stands in place of is measured no more.
      if (how === "into" && into && into.above >= 0 && into.height > 0) {
        const taken = Math.max(1, made.end - made.start);
        const each = into.height / Math.max(1, span);
        this.measured.forget(gap.start, gap.end);
        if (made.start > gap.start) this.measured.set(gap.start, made.start, into.above);
        if (gap.end > made.end) this.measured.set(made.end, gap.end, Math.max(0, into.height - into.above - each * taken));
      } else this.measured.forget(made.start, made.end);
    } catch (error) {
      this.readingFailure = String(error);
    } finally {
      this.meanwhile.delete(since);
      this.reading.delete(gap.key);
      this.structure += 1;
      this.changed();
    }
  }

  /** Nothing was read where the reader stood in `gap`: it is read from its
   *  end instead, as one the reader came to from below. */
  private async fromEnd(gap: Gap): Promise<void> {
    this.reading.delete(gap.key);
    await this.fillPlaces(gap, "end");
  }

  /** Lets go of an earlier part, which becomes a gap again, as tall as
   *  `height` when the page measured it. */
  release(part: Held, height?: number): void {
    const at = this.parts.indexOf(part);
    if (at < 0 || at === this.parts.length - 1) return;
    const span = this.spans.get(part);
    if (span) {
      const earlier = this.parts[at - 1];
      const earlierSpan = earlier && this.spans.get(earlier);
      if (earlierSpan) earlierSpan.joined = false;
      // What lay before it is before the part after it now.
      const later = this.parts[at + 1];
      const laterSpan = later && this.spans.get(later);
      if (laterSpan && span.joined && laterSpan.older === null && span.places.length) laterSpan.older = laterSpan.places[0] ?? null;
      if (height !== undefined) this.measured.set(part.start, part.end, height);
      this.spans.delete(part);
    }
    this.parts.splice(at, 1);
    this.parts = [...this.parts];
    this.structure += 1;
  }

  /** Lets go of what the latest part holds before its latest `events`
   *  events, where it can be cut there. */
  async trim(events: number): Promise<void> {
    this.typed(false);
    const tail = this.tail;
    const span = this.spans.get(tail);
    if (span) {
      // Cut where a stretch begins, so what stays shows whole: the first
      // such place after the one asked for, else the last before it; and
      // where the part holds no such place and twice what it may, where it
      // was asked, since a stretch that long is cut somewhere.
      const asked = span.events.length - events;
      if (asked <= 0) return;
      let cut = asked;
      while (cut < span.events.length && !begins(span.events[cut])) cut += 1;
      if (cut >= span.events.length) {
        cut = asked;
        while (cut > 0 && !begins(span.events[cut])) cut -= 1;
      }
      if (cut <= 0) {
        if (span.events.length < events * 2) return;
        cut = asked;
      }
      const gone = span.keys.slice(0, cut);
      for (let at = 0; at < cut; at += 1) span.deferred.delete(span.keys[at]!);
      span.events.splice(0, cut);
      span.keys.splice(0, cut);
      span.places.splice(0, cut);
      span.older = span.places[0] ?? null;
      const earlier = this.parts[this.parts.length - 2];
      const earlierSpan = earlier && this.spans.get(earlier);
      if (earlierSpan) earlierSpan.joined = false;
      tail.fold.evictPlaces(gone);
      this.placed(tail, span);
      this.parts = [...this.parts];
      this.structure += 1;
      return;
    }
    const source = this.source;
    if (!source) return;
    const generation = this.generation;
    const cut = await source.cut(generation, events).catch(() => null);
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
    if (this.replica) {
      if (!source?.page) throw new Error("this source cannot export the complete conversation");
      // From its first event on, a page after another, none of it held.
      let from: ReadFrom = { from: "after", place: { order: "", item: "00" } };
      for (;;) {
        const read = await source.page(generation, from);
        if (!read) {
          yield { restart: true };
          return;
        }
        const fold = new Fold("export");
        fold.thoughtWords = this.thoughts;
        fold.history(read.events.map((shown) => shown.event), [], read.events.map((shown) => shown.key), false, []);
        yield fold.entries;
        for (const shown of read.events) if (shown.deferred) yield { previewBytes: shown.deferred.bytes };
        if (!read.newer || read.events.length === 0) return;
        from = { from: "after", place: read.newer };
      }
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

  /** What names the gap between two parts of a conversation shown from
   *  its timeline: `earlier` none where nothing is held before `later`. */
  private gapKey(earlier: Held | null, later: Held): string {
    const edge = (part: Held | null, last: boolean): string => {
      const span = part && this.spans.get(part);
      if (!span) return "^";
      return (last ? span.keys[span.keys.length - 1] : span.keys[0]) ?? "-";
    };
    return `gap:${edge(earlier, true)}:${edge(later, false)}`;
  }

  /** What is laid out: each part's blocks, gaps where turns are not held.
   *  A fork's record begins with what it carried over: from the part that
   *  holds the point it was made at, the card there stands for all before. */
  blocks(): Block[] {
    const version = `${this.structure}|${this.parts.map((part) => part.fold.clock).join(",")}`;
    if (this.cache?.version === version) return this.cache.blocks;
    const blocks: Block[] = [];
    if (this.replica) {
      let earlier: Held | null = null;
      for (const part of this.parts) {
        const span = this.spans.get(part);
        if (!span) continue;
        const earlierSpan = earlier ? this.spans.get(earlier) ?? null : null;
        const open = earlierSpan ? !earlierSpan.joined : span.older !== null;
        if (open) {
          const key = this.gapKey(earlier, part);
          const start = earlier ? earlier.end : 0;
          const end = Math.max(start, part.start);
          const loading = this.reading.has(key);
          const measured = this.measured.of(start, end);
          blocks.push({
            kind: "gap", key, stamp: Math.round(measured.px) * 4 + Math.round(measured.rest) * 2 + (loading ? 1 : 0),
            start, end, turns: 0, first: earlier === null, loading, paged: true, measured: measured.px, unmeasured: measured.rest,
          });
        }
        const own = part.fold.blocks();
        // A group the part begins or ends within says so: the rest of it is
        // in what is not held beside it.
        const within = open && !begins(span.events[0]);
        const later = this.parts[this.parts.indexOf(part) + 1];
        const cut = !span.last && !span.joined && !!later && member(span.events[span.events.length - 1]);
        for (const [at, block] of own.entries()) {
          if (block.kind === "activity-group" && ((within && at === 0) || (cut && at === own.length - 1))) {
            blocks.push({ ...block, partialBefore: within && at === 0, partialAfter: cut && at === own.length - 1 });
          } else blocks.push(block);
        }
        earlier = part;
      }
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
  /** Whether a turn's end has something to show in the conversation's own
   *  view too, though the turn ended as it should: the files it changed
   *  (`page.ts`). */
  endKept: ((turn: string) => boolean) | null = null;

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
          return block.end.how === "failed" || !!this.endKept?.(block.turn);
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

  /** The calls of `turn`, in the order they came; with no turn named, of
   *  every turn the page holds, the earliest part first. */
  toolsOf(turn: string | null): Extract<Entry, { kind: "tool" }>[] {
    const tools = (entries: readonly Entry[]) => entries.filter((entry): entry is Extract<Entry, { kind: "tool" }> => entry.kind === "tool");
    // A turn may lie across parts: its calls are all of theirs, in order.
    if (turn !== null) return this.parts.flatMap((part) => (part.fold.holds(turn) ? tools(part.fold.turnEntries(turn)) : []));
    return this.parts.flatMap((part) => tools(part.fold.entries));
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
    // The part a turn begins in is not the one it ends in where it lies
    // across parts: its words are looked for in each.
    for (const part of this.parts) {
      const words = part.fold.entries.find((entry): entry is Heard => entry.kind === "heard" && entry.turn === turn && (!entry.from || entry.from.kind === "user"));
      if (words) return words;
    }
    return null;
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
  /** The start the first words kept wait on, as its machine tells it. */
  starting(): StartShown | null {
    return this.live.starting();
  }
  /** The turn shown under way, the latest to begin; null when none is. */
  runningTurn(): string | null {
    return this.live.runningTurn();
  }
  /** The words on their way a stop pressed now is for (`Fold.coming`). */
  coming(keys?: readonly string[]): { keys: string[]; words: string[]; unnamed: number } {
    return this.live.coming(keys);
  }
  /** The ids of the words a stop waits for, and how many have none yet
   *  (`Fold.haltedIds`, `Fold.haltedUnnamed`). */
  haltedIds(): string[] {
    return this.live.haltedIds();
  }
  haltedUnnamed(): number {
    return this.live.haltedUnnamed();
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
  now(busy: boolean, turn: string | null, kept: KeptWords[], halted: string | null = null): void {
    this.nowTold = { busy, turn, kept, halted };
    this.live.now(busy, turn, kept, halted);
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
    this.tail.end += 1;
  }
}
