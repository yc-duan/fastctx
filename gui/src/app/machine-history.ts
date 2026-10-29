// What a machine's page has read of it, kept while the interface stays open,
// so going back to the page finds its drawings where they were: each
// reading's rates for the last minutes, and the lines' history, which the
// machine sends whole once and then only adds to. Nothing is read or kept
// for a machine whose page was not opened.

import type { LimitReading, Line, LineChange, MachineStats, StatsSeen, Trip } from "../world/stats";

/** One reading's rates, for the drawings. */
export interface Sample {
  /** By the machine's clock, in milliseconds. */
  at: number;
  cpu: number | null;
  memory: number | null;
  gpus: (number | null)[];
  /** Bytes a second each card received and sent, by its name. */
  cards: Record<string, [number, number]>;
}

export interface Kept {
  samples: Sample[];
  /** The last reading, whole, its lines carrying all that was kept. */
  last: MachineStats | null;
  /** When this device last heard from it, by this device's clock. */
  heard: number;
  /** How many readings were asked for, and which of them was taken last:
   *  the order they were asked in says which is newer, whatever the
   *  machine's clock says. */
  asked: number;
  taken: number;
}

/** How far back the drawings of rates reach: sixty readings, two seconds
 *  apart, each a step a hand can see. */
export const SPAN_MS = 2 * 60_000;
/** Kept a little longer than drawn, for a page come back to. */
const KEEP_MS = 15 * 60_000;
/** The machine keeps an hour of round trips and a month of changes. */
const TRIPS_MS = 3_600_000;
const CHANGES_MS = 30 * 86_400_000;
/** It keeps a day of readings of its limits. */
export const LIMITS_MS = 86_400_000;

const kept = new Map<string, Kept>();

/** What was kept of `machine`, empty when nothing was. */
export function keptOf(machine: string): Kept {
  let one = kept.get(machine);
  if (!one) {
    one = { samples: [], last: null, heard: 0, asked: 0, taken: 0 };
    kept.set(machine, one);
  }
  return one;
}

const time = (at: string) => Date.parse(at);

/** What `stats` told of its lines, for the next reading to go on from;
 *  none of a node that does not count them. */
export function seenOf(stats: MachineStats | null): StatsSeen | null {
  if (!stats?.run || stats.lines.some((line) => !line.kept)) return null;
  return { run: stats.run, lines: stats.lines.map((line) => [line.hub, line.kept!]) };
}

/** Takes in a reading of `machine`, the `asked`th asked for: its rates
 *  join the drawings, and, when it was asked for what came after the last
 *  one, its lines' round trips and changes join those kept. The reading kept
 *  is returned. */
export function take(machine: string, stats: MachineStats, added: boolean, asked: number): MachineStats {
  const one = keptOf(machine);
  // One asked before the reading taken last crossed it on the way.
  if (one.last && asked < one.taken) return one.last;
  one.taken = asked;
  const at = time(stats.at);
  const lines = added && one.last ? stats.lines.map((line) => merged(line, one.last!.lines.find((before) => before.hub === line.hub), at)) : stats.lines.map((line) => merged(line, undefined, at));
  const limits = new Map<string, LimitReading>();
  for (const reading of [...(added && one.last ? (one.last.limits ?? []) : []), ...(stats.limits ?? [])]) if (time(reading.at) > at - LIMITS_MS) limits.set(reading.at, reading);
  const whole: MachineStats = { ...stats, lines, limits: [...limits.values()].sort((a, b) => time(a.at) - time(b.at)) };
  one.last = whole;
  one.heard = Date.now();
  one.samples.push(sampleOf(whole, at));
  const from = at - KEEP_MS;
  while (one.samples.length && one.samples[0]!.at < from) one.samples.shift();
  return whole;
}

/** A line as now told, with what was kept of it before. */
function merged(line: Line, before: Line | undefined, at: number): Line {
  const trips = new Map<string, Trip>();
  for (const trip of [...(before?.trips ?? []), ...line.trips]) if (time(trip.at) > at - TRIPS_MS) trips.set(`${trip.at}|${trip.ms}`, trip);
  const changes = new Map<string, LineChange>();
  for (const change of [...(before?.changes ?? []), ...line.changes]) if (time(change.at) > at - CHANGES_MS) changes.set(`${change.kind}|${change.at}`, change);
  return {
    ...line,
    trips: [...trips.values()].sort((a, b) => time(a.at) - time(b.at)),
    changes: [...changes.values()].sort((a, b) => time(a.at) - time(b.at)),
  };
}

/** A stretch of a line's days: up, down (with why, as the node said), or
 *  before anything was recorded. */
export interface Stretch {
  kind: "up" | "down" | "none";
  from: number;
  to: number;
  why: string;
}

/** A moment on a line: it dropped, or the node started. */
export interface Moment {
  kind: "drop" | "start";
  at: number;
  why: string;
}

/** A line's days from `from` to `now`, as its changes tell them. The node
 *  starting means whatever came before ended unseen: a line up before it
 *  is taken as up until then, and it is down until it comes up again. What
 *  the line says of itself now stands over a history that ends early. */
export function daysOf(line: Line, from: number, now: number): { stretches: Stretch[]; moments: Moment[] } {
  const stretches: Stretch[] = [];
  const moments: Moment[] = [];
  let state: Stretch["kind"] = "none";
  let since = -Infinity;
  let why = "";
  const close = (until: number) => {
    const start = Math.max(since, from);
    const end = Math.min(until, now);
    if (end <= start) return;
    const previous = stretches[stretches.length - 1];
    if (previous && previous.kind === state && previous.to === start && previous.why === why) previous.to = end;
    else stretches.push({ kind: state, from: start, to: end, why });
  };
  for (const change of line.changes) {
    const at = time(change.at);
    if (at > now) break;
    close(at);
    since = at;
    if (change.kind === "up") {
      state = "up";
      why = "";
      continue;
    }
    state = "down";
    why = change.kind === "down" ? (change.why ?? "") : "";
    if (at >= from) moments.push({ kind: change.kind === "down" ? "drop" : "start", at, why });
  }
  const told = time(line.since);
  if ((state === "up") !== line.up && told > since && told <= now) {
    close(told);
    since = told;
    state = line.up ? "up" : "down";
    why = "";
  }
  close(now);
  return { stretches, moments };
}

/** How a line held over its stretches: the share of the recorded time it
 *  was up, how often it dropped, and its longest time down. */
export function heldOf(stretches: Stretch[], moments: Moment[]): { online: number | null; drops: number; longest: number } {
  let up = 0;
  let down = 0;
  let longest = 0;
  for (const stretch of stretches) {
    const length = stretch.to - stretch.from;
    if (stretch.kind === "up") up += length;
    else if (stretch.kind === "down") {
      down += length;
      longest = Math.max(longest, length);
    }
  }
  return { online: up + down > 0 ? up / (up + down) : null, drops: moments.filter((moment) => moment.kind === "drop").length, longest };
}

/** A line's round trips over the hour: the last, the median and the
 *  highest, in milliseconds. */
export function tripsHeld(line: Line): { last: number | null; median: number | null; highest: number | null } {
  if (!line.trips.length) return { last: null, median: null, highest: null };
  const sorted = line.trips.map((trip) => trip.ms).sort((a, b) => a - b);
  return { last: line.trips[line.trips.length - 1]!.ms, median: sorted[Math.floor(sorted.length / 2)]!, highest: sorted[sorted.length - 1]! };
}

function sampleOf(stats: MachineStats, at: number): Sample {
  const cards: Record<string, [number, number]> = {};
  for (const card of stats.cards) if (card.up) cards[card.name] = [card.receiving ?? 0, card.sending ?? 0];
  return {
    at,
    cpu: stats.cpu.busy ?? null,
    memory: stats.memory.total ? stats.memory.used / stats.memory.total : null,
    gpus: (stats.gpus ?? []).map((gpu) => gpu.busy ?? null),
    cards,
  };
}
