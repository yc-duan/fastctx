// What a login's usage report says, in the shapes the page draws: each
// person once, named and inked as this reader knows them, the days and
// hours by this device's clock, and the week's windows as a line through
// time. Dollars are Claude Code's own ledger; Codex keeps none, so its
// usage is counted in tokens alone.

import type { LimitWindow } from "../../world/agents";
import type { KindsUsd, ModelUse, Tokens, UsageReport, UsageSum } from "../../world/usage";
import type { HubAccount } from "../../world/accounts";

export type Unit = "usd" | "tokens";

/** Who a report's account is to this reader: one key per person, so the
 *  machine's own sessions and its owner's are one. */
export interface Named {
  key: string;
  name: string;
  me: boolean;
}

/** One person whose use the report holds, as the page shows them. */
export interface Person extends Named {
  index: number;
  /** The CSS colour it is drawn in. */
  ink: string;
  week: Sum;
}

export interface Sum {
  tokens: number;
  /** Dollars; null where the agent keeps no cost ledger. */
  usd: number | null;
}

export interface Hour extends Sum {
  at: number;
  person: number;
}

export interface Week {
  start: number;
  end: number;
  /** The login's own seven-day window, not the last seven days. */
  window: boolean;
  models: { model: string; sum: Sum }[];
  tokens: Required<Tokens>;
  kinds: { input: number; cache_read: number; cache_write: number; output: number } | null;
}

export interface Usage {
  people: Person[];
  hours: Hour[];
  week: Week;
  readings: { at: number; windows: LimitWindow[] }[];
  /** Whether it is counted in dollars too. */
  usd: boolean;
}

/** The inks people are drawn in, the reader's own first. */
const INKS = ["var(--pen-clay)", "var(--pen-sky)", "var(--pen-sage)", "var(--pen-rose)", "var(--pen-graphite)"];

export const DAY_MS = 86_400_000;
export const HOUR_MS = 3_600_000;

const dollarsOf = (millionths: number | undefined): number | null => (millionths === undefined ? null : millionths / 1_000_000);
const sumOf = (sum: UsageSum): Sum => ({ tokens: sum.tokens, usd: dollarsOf(sum.usd) });
const add = (a: Sum, b: Sum): Sum => ({ tokens: a.tokens + b.tokens, usd: a.usd === null && b.usd === null ? null : (a.usd ?? 0) + (b.usd ?? 0) });

/** The report as this reader sees it: its people named by `nameOf`, those
 *  named alike merged, the reader first and the rest by what they used
 *  this week. */
export function usageOf(report: UsageReport, nameOf: (account: HubAccount | null) => Named): Usage {
  const named: { named: Named; week: Sum }[] = [];
  const into: number[] = report.people.map((person) => {
    const one = nameOf(person.account);
    const found = named.findIndex((known) => known.named.key === one.key);
    if (found >= 0) {
      named[found]!.week = add(named[found]!.week, sumOf(person.week));
      return found;
    }
    named.push({ named: one, week: sumOf(person.week) });
    return named.length - 1;
  });
  const order = named.map((_, index) => index).sort((a, b) => {
    const [x, y] = [named[a]!, named[b]!];
    return Number(y.named.me) - Number(x.named.me) || (y.week.usd ?? 0) - (x.week.usd ?? 0) || y.week.tokens - x.week.tokens;
  });
  const place = new Map(order.map((was, now) => [was, now]));
  const people = order.map((was, index): Person => ({ ...named[was]!.named, index, ink: INKS[index % INKS.length]!, week: named[was]!.week }));
  const hours = new Map<string, Hour>();
  for (const hour of report.hours) {
    const person = place.get(into[hour.person] ?? -1);
    if (person === undefined) continue;
    const at = Date.parse(hour.at);
    const key = `${at}:${person}`;
    const known = hours.get(key);
    const sum = sumOf(hour);
    hours.set(key, known ? { ...known, ...add(known, sum) } : { at, person, ...sum });
  }
  const now = Date.now();
  const week = report.week;
  const kinds = week.kinds_usd;
  return {
    people,
    hours: [...hours.values()].sort((a, b) => a.at - b.at),
    week: {
      start: week.start ? Date.parse(week.start) : now - 7 * DAY_MS,
      end: week.end ? Date.parse(week.end) : now,
      window: week.window,
      models: week.models.map((one: ModelUse) => ({ model: one.model, sum: sumOf(one) })),
      tokens: { input: 0, cache_read: 0, cache_write: 0, output: 0, reasoning: 0, ...week.tokens },
      kinds: kinds ? kindsOf(kinds) : null,
    },
    readings: report.readings.map((reading) => ({ at: Date.parse(reading.at), windows: reading.windows })),
    usd: report.hours.some((hour) => hour.usd !== undefined) || report.people.some((person) => person.week.usd !== undefined),
  };
}

function kindsOf(kinds: KindsUsd): Week["kinds"] {
  return {
    input: kinds.input / 1_000_000,
    cache_read: kinds.cache_read / 1_000_000,
    cache_write: kinds.cache_write / 1_000_000,
    output: kinds.output / 1_000_000,
  };
}

/** An amount in the unit. */
export function valueOf(sum: Sum, unit: Unit): number {
  return unit === "usd" ? (sum.usd ?? 0) : sum.tokens;
}

/** Local midnight of the day `at` is in. */
export function dayOf(at: number): number {
  const date = new Date(at);
  return new Date(date.getFullYear(), date.getMonth(), date.getDate()).getTime();
}

/** Local midnight `days` calendar days before `day`, counted by the
 *  calendar so a change of daylight saving never moves a day. */
export function daysBefore(day: number, days: number): number {
  const date = new Date(day);
  return new Date(date.getFullYear(), date.getMonth(), date.getDate() - days).getTime();
}

/** Monday 0 to Sunday 6. */
export function weekdayOf(at: number): number {
  return (new Date(at).getDay() + 6) % 7;
}

/** What each person used on each local day: day → amount by person. */
export function byDay(usage: Usage, unit: Unit, only: number | null = null): Map<number, Map<number, number>> {
  const days = new Map<number, Map<number, number>>();
  for (const hour of usage.hours) {
    if (only !== null && hour.person !== only) continue;
    const value = valueOf(hour, unit);
    if (!value) continue;
    const day = dayOf(hour.at);
    const people = days.get(day) ?? new Map<number, number>();
    people.set(hour.person, (people.get(hour.person) ?? 0) + value);
    days.set(day, people);
  }
  return days;
}

export function total(people: Map<number, number> | undefined): number {
  let sum = 0;
  for (const value of people?.values() ?? []) sum += value;
  return sum;
}

/** What was used from `from` up to `to`, by person. */
export function between(usage: Usage, unit: Unit, from: number, to: number, only: number | null = null): Map<number, number> {
  const people = new Map<number, number>();
  for (const hour of usage.hours) {
    if (hour.at < from || hour.at >= to || (only !== null && hour.person !== only)) continue;
    const value = valueOf(hour, unit);
    if (value) people.set(hour.person, (people.get(hour.person) ?? 0) + value);
  }
  return people;
}

/** What was used in each hour of each weekday over the last `days` days:
 *  seven rows of 24, Monday first. */
export function byHourOfWeek(usage: Usage, unit: Unit, now: number, days: number, only: number | null = null): number[][] {
  const grid = Array.from({ length: 7 }, () => Array<number>(24).fill(0));
  const since = daysBefore(dayOf(now), days - 1);
  for (const hour of usage.hours) {
    if (hour.at < since || (only !== null && hour.person !== only)) continue;
    grid[weekdayOf(hour.at)]![new Date(hour.at).getHours()]! += valueOf(hour, unit);
  }
  return grid;
}

/** A window's share used at each reading, 0 to 1 and past it. */
export interface Track {
  points: [number, number][];
  last: number;
  resets: number | null;
}

/** How the window `minutes` long went through the week, from the readings
 *  that had it. */
export function track(usage: Usage, minutes: number): Track | null {
  const points: [number, number][] = [];
  let resets: number | null = null;
  for (const reading of usage.readings) {
    const window = reading.windows.find((one) => one.minutes === minutes || (minutes === 300 ? one.name.startsWith("five_hour") : one.name.startsWith("seven_day")));
    if (!window) continue;
    points.push([reading.at, window.used_tenths / 1000]);
    resets = window.resets_at ? Date.parse(window.resets_at) : resets;
  }
  if (!points.length) return null;
  return { points, last: points[points.length - 1]![1], resets };
}

/** How fast the seven-day window filled over the last day of readings, as
 *  a share an hour; none when the readings span too little of it. */
export function pace(week: Track, now: number): number | null {
  const recent = week.points.filter(([at]) => at >= now - DAY_MS);
  if (recent.length < 2) return null;
  const [firstAt, first] = recent[0]!;
  const [lastAt, last] = recent[recent.length - 1]!;
  if (lastAt - firstAt < 3 * HOUR_MS || last <= first) return null;
  return ((last - first) / (lastAt - firstAt)) * HOUR_MS;
}

/** The longest run of days in a row it was used, over the last `span`
 *  days up to `today`. */
export function longestRun(days: Map<number, Map<number, number>>, today: number, span: number): number {
  let longest = 0;
  let run = 0;
  for (let back = span - 1; back >= 0; back -= 1) {
    run = total(days.get(daysBefore(today, back))) > 0 ? run + 1 : 0;
    longest = Math.max(longest, run);
  }
  return longest;
}

/** Thresholds splitting the days used into four levels by quartile, so a
 *  light user's calendar is as legible as a heavy one's. */
export function levels(values: number[]): (value: number) => number {
  const sorted = values.filter((value) => value > 0).sort((a, b) => a - b);
  const at = (share: number) => sorted[Math.floor(share * (sorted.length - 1))] ?? 0;
  const cuts = [at(0.25), at(0.5), at(0.8)];
  return (value) => (!value ? 0 : value <= cuts[0]! ? 1 : value <= cuts[1]! ? 2 : value <= cuts[2]! ? 3 : 4);
}
