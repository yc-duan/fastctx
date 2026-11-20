// A conversation shown from its timeline (`src-tauri/src/replica.rs`): the
// page reads parts of it where its reader is, each a run of events in the
// timeline's order, and keeps what it holds current by the records the
// timeline gains. An event stands at a place: an order key, compared as
// text, and the item it is. Keys are given a fixed space apart as events
// are added at the end (`fastctx-timeline-core`, order), so how far apart two
// places are is very nearly how many events lie between them: what is not
// held is laid out by that, as tall as so many events usually are.

import type { SessionEvent } from "../world/events";

export interface Place { order: string; item: string }

/** One event of the timeline as the shell hands it. */
export interface Shown {
  key: string;
  place: Place;
  event: SessionEvent;
  /** What holds it whole when the event here is cut short. */
  deferred?: { bytes: number; object: string };
}

/** A part of the timeline as the shell read it. */
export interface ReplicaPart {
  generation: number;
  events: Shown[];
  /** Where to read on from towards the start; none where it begins. */
  older: Place | null;
  /** Where to read on from towards the end; none where it ends. */
  newer: Place | null;
  /** Whether the timeline is still being made from the history. */
  preparing: boolean;
  /** Revision and cursor of a single reading. */
  stamp?: string;
}

export type ReplicaRecord = { record: "is"; shown: Shown } | { record: "gone"; key: string };

export type ReadFrom = { from: "latest" } | { from: "before"; place: Place } | { from: "after"; place: Place };

/** What a part read from the timeline holds besides its fold. */
export interface Span {
  /** Its events as given, in order: cut short where one is deferred. */
  events: SessionEvent[];
  keys: string[];
  places: Place[];
  deferred: Map<string, { bytes: number; object: string }>;
  /** Where to read on before it; none where the timeline begins. */
  older: Place | null;
  /** Whether it is the timeline's end: what is added there comes to it. */
  last: boolean;
  /** Whether the part after it begins just where this one ends. */
  joined: boolean;
}

/** The space left between two order keys as they are first given. */
const SPACE = 2 ** 20;

/** How far along the timeline a place stands, counted in events: its
 *  order key over the space between two. */
export function ordinal(place: Place): number {
  const hex = `${place.order}0000000000000000`.slice(0, 16);
  const high = parseInt(hex.slice(0, 8), 16) || 0;
  const low = parseInt(hex.slice(8, 16), 16) || 0;
  return high * 4096 + low / SPACE;
}

/** A place to read on from, just before whatever stands `ordinal` events
 *  along the timeline. */
export function placeAt(ordinal: number): Place {
  const key = Math.max(0, Math.floor(ordinal * SPACE));
  const high = Math.floor(key / 2 ** 32);
  const low = key % 2 ** 32;
  return { order: high.toString(16).padStart(8, "0") + low.toString(16).padStart(8, "0"), item: "00" };
}

/** Whether `a` stands before `b`. */
export function before(a: Place, b: Place): boolean {
  return a.order < b.order || (a.order === b.order && a.item < b.item);
}

/** Whether a stretch of the conversation begins at `event`: a turn, the
 *  user's words, or the agent's own (`replica.rs`, boundary). */
export function begins(event: SessionEvent | undefined): boolean {
  return !!event && (event.event === "turn_started" || event.event === "heard" || (event.event === "finished" && event.item.kind === "message"));
}

/** Whether `event` is of the work shown as one group between two such
 *  beginnings. */
export function member(event: SessionEvent | undefined): boolean {
  return !!event && (event.event === "finished" || event.event === "began" || event.event === "updated")
    && (event.item.kind === "tool" || event.item.kind === "thinking" || event.item.kind === "agent_activity");
}

export function spanOf(part: ReplicaPart): Span {
  const deferred = new Map<string, { bytes: number; object: string }>();
  for (const shown of part.events) if (shown.deferred) deferred.set(shown.key, shown.deferred);
  return {
    events: part.events.map((shown) => shown.event),
    keys: part.events.map((shown) => shown.key),
    places: part.events.map((shown) => shown.place),
    deferred,
    older: part.older,
    last: part.newer === null,
    joined: false,
  };
}

/** Takes the event at `at` out of `span`. */
export function removeAt(span: Span, at: number): void {
  span.deferred.delete(span.keys[at]!);
  span.events.splice(at, 1);
  span.keys.splice(at, 1);
  span.places.splice(at, 1);
}

/** Puts `shown` into `span` where its place stands. */
export function insert(span: Span, shown: Shown): void {
  let at = span.places.length;
  while (at > 0 && before(shown.place, span.places[at - 1]!)) at -= 1;
  span.events.splice(at, 0, shown.event);
  span.keys.splice(at, 0, shown.key);
  span.places.splice(at, 0, shown.place);
  if (shown.deferred) span.deferred.set(shown.key, shown.deferred);
  else span.deferred.delete(shown.key);
}

/** Heights the page measured of stretches of the timeline it no longer
 *  holds, by where they stand: a gap is as tall as what was measured of it,
 *  and as tall as so many events usually are for the rest. */
export class Measured {
  private runs: { start: number; end: number; px: number }[] = [];

  clear(): void { this.runs = []; }

  /** `start..end` was `px` tall. */
  set(start: number, end: number, px: number): void {
    if (!(end > start) || !(px >= 0)) return;
    this.forget(start, end);
    this.runs.push({ start, end, px });
    this.runs.sort((a, b) => a.start - b.start);
  }

  /** Nothing is known of `start..end` any more: what was measured across
   *  either end keeps its share of what lies outside. */
  forget(start: number, end: number): void {
    const kept: typeof this.runs = [];
    for (const run of this.runs) {
      if (run.end <= start || run.start >= end) { kept.push(run); continue; }
      const each = run.px / (run.end - run.start);
      if (run.start < start) kept.push({ start: run.start, end: start, px: each * (start - run.start) });
      if (run.end > end) kept.push({ start: end, end: run.end, px: each * (run.end - end) });
    }
    this.runs = kept;
  }

  /** Of `start..end`: the pixels measured, and how many events' worth of it
   *  was never measured. */
  of(start: number, end: number): { px: number; rest: number } {
    let px = 0;
    let covered = 0;
    for (const run of this.runs) {
      const from = Math.max(start, run.start);
      const to = Math.min(end, run.end);
      if (to <= from) continue;
      px += run.px * (to - from) / (run.end - run.start);
      covered += to - from;
    }
    return { px, rest: Math.max(0, end - start - covered) };
  }
}
