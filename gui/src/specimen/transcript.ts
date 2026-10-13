// What the shell keeps of a conversation (`src-tauri/src/transcript.rs`),
// kept in the page for a specimen, which has no shell: the same cuts
// between turns and the same parts, so a specimen of thousands of turns is
// read by its page as a live conversation is.

import type { SessionEvent } from "../world/events";
import type { Cut, Edge, FindDoc, Found, PartRead, PartSource, Window } from "../conversation/conversation";

const PART_EVENTS = 400;
const PART_BYTES = 1 << 20;

export class LocalTranscript implements PartSource {
  private generation = 0;
  private events: SessionEvent[] = [];
  private bytes: number[] = [0];
  private units: number[] = [0];
  private first = new Map<string, number>();
  private previousStarted = false;
  private tools: [number, string][] = [];
  private seenTools = new Set<string>();
  private facts = new Map<string, number[]>();
  /** The call that started each of the host's own agents. */
  private agentCalls = new Map<string, string>();
  private titled: number | null = null;
  private context: number | null = null;
  private answered: number | null = null;
  private agents: number | null = null;
  private texts = new Map<number, { end: number; docs: FindDoc[] }>();

  /** `delay`: how long a part takes to come, as a shell's would. */
  constructor(history: readonly SessionEvent[], private readonly delay = 30) {
    for (const event of history) this.admit(event);
  }

  /** The latest part, as a page opens on it. */
  window(): Window {
    const start = this.latest(PART_EVENTS, PART_BYTES);
    const carry = [...new Set([this.titled, this.context, this.answered, this.agents].filter((at): at is number => at !== null && at < start))]
      .sort((a, b) => a - b)
      .map((at) => this.events[at]!);
    return { generation: this.generation, start, turns_before: this.turnsBefore(start), events: this.events.slice(start), carry };
  }

  /** One more event; its place. */
  add(event: SessionEvent): number {
    this.admit(event);
    return this.events.length - 1;
  }

  private admit(event: SessionEvent): void {
    const index = this.events.length;
    this.events.push(event);
    this.bytes.push(this.bytes[index]! + JSON.stringify(event).length);
    let fresh = true;
    let earliest: number | null = null;
    for (const name of names(event)) {
      const at = this.first.get(name);
      if (at === undefined) this.first.set(name, index);
      else {
        fresh = false;
        earliest = earliest === null ? at : Math.min(earliest, at);
      }
    }
    if (earliest !== null) while (this.units.length > 1 && this.units[this.units.length - 1]! > earliest) this.units.pop();
    const starts = event.event === "turn_started" || (event.event === "heard" && !this.previousStarted);
    if (starts && fresh && index > this.units[this.units.length - 1]!) this.units.push(index);
    this.previousStarted = event.event === "turn_started";
    if ((event.event === "began" || event.event === "updated" || event.event === "finished") && (event.item.kind === "tool" || event.item.kind === "agent_activity") && !this.seenTools.has(event.item.id)) {
      this.seenTools.add(event.item.id);
      this.tools.push([index, event.item.id]);
    }
    if (event.event === "changed") {
      const change = event.change;
      if (change.kind === "run_facts_updated") this.fact(change.update.call_id, index);
      if (change.kind === "agent_started" && change.agent.call) this.agentCalls.set(change.agent.id, change.agent.call);
      if (change.kind === "agent_started" || change.kind === "agent_event" || change.kind === "agent_ended") {
        const call = this.agentCalls.get(change.kind === "agent_started" ? change.agent.id : change.agent_id);
        if (call) this.fact(call, index);
      }
      if ((change.kind === "model_response" && change.usage) || change.kind === "message_usage") this.answered = index;
      if (change.kind === "agents") this.agents = index;
    }
    if (event.event === "titled") this.titled = index;
    if (event.event === "context") this.context = index;
  }

  /** A fact told of a call, served with the part holding the call. */
  private fact(call: string, index: number): void {
    const told = this.facts.get(call) ?? [];
    told.push(index);
    this.facts.set(call, told);
  }

  private latest(events: number, bytes: number): number {
    const end = this.events.length;
    for (let at = this.units.length - 1; at >= 0; at -= 1) {
      const start = this.units[at]!;
      if (end - start >= events || this.bytes[end]! - this.bytes[start]! >= bytes) return start;
    }
    return 0;
  }

  private turnsBefore(at: number): number {
    let low = 0;
    let high = this.units.length;
    while (low < high) {
      const middle = (low + high) >> 1;
      if (this.units[middle]! < at) low = middle + 1;
      else high = middle;
    }
    return low;
  }

  private turnOf(at: number): number {
    const index = Math.max(1, this.turnsBefore(at + 1)) - 1;
    return this.units[index]!;
  }

  private big(start: number, end: number, share: number): boolean {
    return end - start >= PART_EVENTS / share || this.bytes[end]! - this.bytes[start]! >= PART_BYTES / share;
  }

  private before(end: number, floor: number, share: number): number {
    for (let at = this.turnsBefore(end) - 1; at >= 0; at -= 1) {
      const start = this.units[at]!;
      if (start <= floor) return floor;
      if (this.big(start, end, share)) return start;
    }
    return floor;
  }

  private after(start: number, ceiling: number, share: number): number {
    for (let at = this.turnsBefore(start + 1); at < this.units.length; at += 1) {
      const end = this.units[at]!;
      if (end >= ceiling) return ceiling;
      if (this.big(start, end, share)) return end;
    }
    return ceiling;
  }

  async part(generation: number, edge: Edge): Promise<PartRead | null> {
    if (generation !== this.generation) return null;
    const len = this.events.length;
    let start: number;
    let end: number;
    if (edge.edge === "before") {
      end = Math.min(edge.at, len);
      start = this.before(end, Math.min(edge.floor, end), 1);
    } else if (edge.edge === "after") {
      const ceiling = Math.min(edge.ceiling, len);
      start = Math.min(edge.at, ceiling);
      end = this.after(start, ceiling, 1);
    } else {
      const ceiling = Math.min(edge.ceiling, len);
      const floor = Math.min(edge.floor, ceiling);
      const own = Math.min(Math.max(this.turnOf(Math.min(edge.at, ceiling - 1)), floor), ceiling);
      start = this.before(own, floor, 2);
      end = this.after(own, ceiling, 2);
    }
    if (start >= end) return null;
    // A specimen's parts come as a shell's would: after a moment.
    await new Promise((resolve) => setTimeout(resolve, this.delay));
    return {
      generation,
      start,
      end,
      turns_before: this.turnsBefore(start),
      turns: this.turnsBefore(end) - this.turnsBefore(start),
      events: this.events.slice(start, end),
      late: this.late(start, end),
    };
  }

  private late(start: number, end: number): SessionEvent[] {
    const late: number[] = [];
    for (const [at, item] of this.tools) {
      if (at < start || at >= end) continue;
      for (const told of this.facts.get(item) ?? []) if (told >= end) late.push(told);
    }
    return late.sort((a, b) => a - b).map((at) => this.events[at]!);
  }

  async locate(generation: number, turn: string): Promise<number | null> {
    return generation === this.generation ? (this.first.get(`t:${turn}`) ?? null) : null;
  }

  async cut(generation: number, events: number): Promise<Cut | null> {
    if (generation !== this.generation) return null;
    const at = this.latest(events, Infinity);
    return { generation, at, turns_before: this.turnsBefore(at) };
  }

  async index(generation: number, start: number, end: number, docs: FindDoc[]): Promise<boolean> {
    if (generation !== this.generation) return false;
    for (const [from, indexed] of this.texts) if (from < end && indexed.end > start) this.texts.delete(from);
    this.texts.set(start, { end, docs });
    return true;
  }

  async find(generation: number, query: string, held: [number, number][]): Promise<Found | null> {
    if (generation !== this.generation) return null;
    const holds = (at: number) => held.some(([from, to]) => from <= at && at < to);
    const ranges: Found["ranges"] = [];
    if (query) {
      for (const [start, indexed] of [...this.texts].sort((a, b) => a[0] - b[0])) {
        const hits: [string, string, number, number][] = [];
        for (const doc of indexed.docs) {
          if (holds(doc.at)) continue;
          let count = 0;
          for (let at = doc.text.indexOf(query); at >= 0; at = doc.text.indexOf(query, at + query.length)) count += 1;
          if (count) hits.push([doc.block, doc.part, count, doc.at]);
        }
        if (hits.length) ranges.push({ start, end: indexed.end, hits });
      }
    }
    const covered: [number, number][] = [...held, ...[...this.texts].map(([start, indexed]) => [start, indexed.end] as [number, number])].sort((a, b) => a[0] - b[0]);
    const unindexed: [number, number][] = [];
    const len = this.events.length;
    let at = 0;
    for (const [start, end] of [...covered, [len, len] as [number, number]]) {
      while (at < Math.min(start, len)) {
        const next = this.after(at, Math.min(start, len), 1);
        unindexed.push([at, next]);
        at = next;
      }
      at = Math.max(at, end);
    }
    return { generation, ranges, unindexed };
  }
}

/** The names an event refers to that must stay in its part, as the shell
 *  reads them. */
function names(event: SessionEvent): string[] {
  const names: string[] = [];
  const turn = (at: string | null | undefined) => {
    if (at) names.push(`t:${at}`);
  };
  switch (event.event) {
    case "turn_started":
    case "turn_ended":
    case "steps":
      turn(event.turn);
      break;
    case "began":
    case "finished":
    case "updated":
      turn(event.turn);
      // Items in no turn are their own, together with the draft they replace.
      if (!event.turn) {
        names.push(`i:${event.item.id}`);
        if (event.item.draft) names.push(`i:${event.item.draft}`);
      }
      break;
    case "grew":
      turn(event.turn);
      if (!event.turn) names.push(`i:${event.item}`);
      break;
    case "heard":
      turn(event.turn);
      if (event.delivery) names.push(`d:${event.delivery}`);
      if (event.message) names.push(`m:${event.message.id}`);
      break;
    case "changed": {
      const change = event.change;
      if (change.kind === "message_usage" || change.kind === "usage") turn(event.turn);
      if (change.kind === "request" || change.kind === "resolved") {
        turn(event.turn);
        names.push(`a:${JSON.stringify(change.id)}`);
      }
      break;
    }
    case "delivery_uncertain":
      names.push(`d:${event.delivery}`);
      break;
  }
  return names;
}
