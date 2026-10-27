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

import type { AskState } from "./ask-state";
import { inOrder, type ModelRequest } from "./requests";
import type { SessionEvent, TurnEnd } from "../world/events";
import type { MachineName } from "../world/events";
import { Fold, sayOf, type Block, type Entry, type Gap, type KeptWords, type SendState, type ShownAttachment } from "./model";
import type { AgentRun } from "./agents";
import type { PendingControl } from "../world/control";

/** The part a page opens on, in place of all it held. */
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

/** Where the parts a page does not hold are read from. */
export interface PartSource {
  part(generation: number, edge: Edge): Promise<PartRead | null>;
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

export class Conversation {
  /** The parts held, in order; the last is the latest, and live. */
  private parts: Held[];
  private generation = 0;
  private source: PartSource | null = null;
  /** Moves when parts come and go. */
  private structure = 0;
  private folds = 0;
  private cache: { version: string; blocks: Block[] } | null = null;
  private chatCache: { version: string; blocks: Block[] } | null = null;
  /** Gaps being read, by key. */
  private reading = new Set<string>();
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
  get context(): { used: number; window: number | null } | null {
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
  get windowed(): boolean {
    return this.source !== null;
  }

  // Reading it from the shell.

  /** The part the page opens on, in place of all it held. */
  open(window: Window, source: PartSource | null): void {
    this.generation = window.generation;
    this.source = source;
    const tail = this.live;
    tail.history(window.events, window.carry);
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
    this.tail.end += 1;
    return true;
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

  /** Reads the turns where `turn` first appears, wherever they are; true
   *  once they are held. */
  async reachTurn(turn: string): Promise<boolean> {
    if (this.foldOf(turn)) return true;
    const source = this.source;
    if (!source) return false;
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
      console.warn("a part of the conversation could not be read", error);
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
  async *everything(): AsyncGenerator<readonly Entry[]> {
    const source = this.source;
    const generation = this.generation;
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

  /** What is laid out: each part's blocks, gaps where turns are not held. */
  blocks(): Block[] {
    const version = `${this.structure}|${this.parts.map((part) => part.fold.clock).join(",")}`;
    if (this.cache?.version === version) return this.cache.blocks;
    const blocks: Block[] = [];
    let end = 0;
    let turns = 0;
    for (const part of this.parts) {
      if (part.start > end) {
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
   *  the user's words, what the agent said to them, what waits on them,
   *  and a failure of what they asked. A turn the user began that ended
   *  with nothing said to them shows its last words instead, as the answer
   *  they were. */
  chatBlocks(): Block[] {
    const blocks = this.blocks();
    const version = this.cache!.version;
    if (this.chatCache?.version === version) return this.chatCache.blocks;
    const asked = new Set<string>();
    const spoke = new Set<string>();
    // A command the user gave begins the next turn, unless words do: the
    // host keeps its own line for the command out of the conversation.
    let commanded = false;
    for (const block of blocks) {
      if (block.kind === "command") {
        commanded ||= block.row.control.do === "command";
        continue;
      }
      if (block.kind === "heard" && block.turn && !block.notice && (!block.from || block.from.kind === "user")) asked.add(block.turn);
      if (block.kind === "say") spoke.add(block.turn);
      const turn = "turn" in block ? block.turn : null;
      if (commanded && turn) {
        if (block.kind !== "heard") asked.add(turn);
        commanded = false;
      }
    }
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
          return block.prose === "message" && asked.has(block.turn) && !spoke.has(block.turn) && this.turnEnded(block.turn) && this.lastMessageOf(block.turn) === block.key;
        case "ended":
          return block.end.how === "failed" && asked.has(block.turn);
        default:
          return false;
      }
    });
    this.chatCache = { version, blocks: shown };
    return shown;
  }

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
    // A turn that said something to the user answered with that.
    if (fold?.spokeIn(turn)) {
      return fold.entries
        .flatMap((entry) => (entry.kind === "tool" && entry.turn === turn ? [sayOf(entry)] : []))
        .flatMap((say) => (say ? [say.text] : []))
        .join("\n\n");
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
