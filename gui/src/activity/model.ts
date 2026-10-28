// The activity of every machine asked, put together (`world-gui.md`, 活动):
// each deed with the machine it ran on, each hour with the machine that
// counted it, and the shapes the views draw from them.

import type { Deed, Hour, Outcome } from "../world/book";

export const HOUR_MS = 3_600_000;
export const DAY_MS = 24 * HOUR_MS;

/** A deed with the machine it ran on. */
export interface Placed extends Deed {
  machine: string;
}

export interface Counted extends Hour {
  machine: string;
}

/** What the page holds of every machine's book. */
export interface Gathered {
  deeds: Placed[];
  hours: Counted[];
  /** Machines asked that did not answer. */
  away: string[];
  /** Whether some machine left deeds out past the limit. */
  more: boolean;
}

export const empty = (): Gathered => ({ deeds: [], hours: [], away: [], more: false });

export const began = (deed: Deed) => Date.parse(deed.began);
export const ended = (deed: Deed) => (deed.ended ? Date.parse(deed.ended) : null);
/** How long a deed ran, or has run until `now`. */
export const lasted = (deed: Deed, now: number) => Math.max(0, (ended(deed) ?? now) - began(deed));
export const failing = (outcome: Outcome) => outcome === "failed" || outcome === "lost";

/** Local midnight of the day holding `at`. */
export function dayOf(at: number): number {
  const date = new Date(at);
  date.setHours(0, 0, 0, 0);
  return date.getTime();
}

/** Monday first. */
export function weekdayOf(at: number): number {
  return (new Date(at).getDay() + 6) % 7;
}

/** Runs and failures by local day, from the hours. */
export function byDay(hours: Counted[]): Map<number, { runs: number; failed: number }> {
  const days = new Map<number, { runs: number; failed: number }>();
  for (const hour of hours) {
    const day = dayOf(Date.parse(hour.at));
    const seen = days.get(day) ?? { runs: 0, failed: 0 };
    seen.runs += hour.runs;
    seen.failed += hour.failed;
    days.set(day, seen);
  }
  return days;
}

/** Runs by weekday and hour of the day, over the `days` before `now`. */
export function byHourOfWeek(hours: Counted[], now: number, days = 90): number[][] {
  const grid = Array.from({ length: 7 }, () => new Array<number>(24).fill(0));
  const from = now - days * DAY_MS;
  for (const hour of hours) {
    const at = Date.parse(hour.at);
    if (at < from) continue;
    grid[weekdayOf(at)]![new Date(at).getHours()]! += hour.runs;
  }
  return grid;
}

/** Totals of `key` over the hours since `from`. */
export function spread(hours: Counted[], from: number, key: (hour: Counted) => string): { key: string; runs: number; failed: number; busy: number }[] {
  const totals = new Map<string, { key: string; runs: number; failed: number; busy: number }>();
  for (const hour of hours) {
    if (Date.parse(hour.at) < from) continue;
    const name = key(hour);
    const seen = totals.get(name) ?? { key: name, runs: 0, failed: 0, busy: 0 };
    seen.runs += hour.runs;
    seen.failed += hour.failed;
    seen.busy += hour.busy;
    totals.set(name, seen);
  }
  return [...totals.values()].sort((a, b) => b.runs - a.runs);
}

/** One session's deeds, as the sessions view lists it. */
export interface SessionSeen {
  session: string;
  agent: Deed["agent"];
  held: boolean;
  /** Where the session lives. */
  home: string;
  first: number;
  last: number;
  runs: number;
  failed: number;
  files: number;
  machines: string[];
  running: boolean;
  /** When each of its deeds began, and whether it went wrong. */
  times: { at: number; failed: boolean }[];
}

export function sessions(deeds: Placed[]): SessionSeen[] {
  const seen = new Map<string, SessionSeen>();
  for (const deed of deeds) {
    if (!deed.session) continue;
    const at = began(deed);
    const home = deed.session.slice(deed.session.lastIndexOf("@") + 1);
    const one = seen.get(deed.session) ?? {
      session: deed.session,
      agent: deed.agent,
      held: !!deed.held,
      home,
      first: at,
      last: at,
      runs: 0,
      failed: 0,
      files: 0,
      machines: [],
      running: false,
      times: [],
    };
    one.agent ??= deed.agent;
    one.held ||= !!deed.held;
    one.first = Math.min(one.first, at);
    one.last = Math.max(one.last, ended(deed) ?? at);
    one.runs += 1;
    if (failing(deed.outcome)) one.failed += 1;
    one.files += deed.touched?.length ?? 0;
    one.running ||= deed.outcome === "running";
    one.times.push({ at, failed: failing(deed.outcome) });
    for (const machine of [deed.machine, ...(deed.went ?? [])]) if (!one.machines.includes(machine)) one.machines.push(machine);
    seen.set(deed.session, one);
  }
  return [...seen.values()].sort((a, b) => b.last - a.last);
}

/** A file a deed touched, with where and when. */
export interface FileSeen {
  machine: string;
  path: string;
  how: NonNullable<Deed["touched"]>[number]["how"];
  added?: number;
  removed?: number;
  seen: boolean;
  at: number;
  job: string;
  session?: string;
}

/** The files touched, the last change of each; grouped by machine and
 *  then by folder. */
export function files(deeds: Placed[]): Map<string, Map<string, FileSeen[]>> {
  const latest = new Map<string, FileSeen>();
  for (const deed of deeds) {
    for (const touched of deed.touched ?? []) {
      const key = `${deed.machine}\n${touched.path}`;
      const at = ended(deed) ?? began(deed);
      const before = latest.get(key);
      if (before && before.at >= at) continue;
      latest.set(key, { machine: deed.machine, ...touched, seen: !!touched.seen, at, job: deed.job, ...(deed.session ? { session: deed.session } : {}) });
    }
  }
  const grouped = new Map<string, Map<string, FileSeen[]>>();
  for (const file of [...latest.values()].sort((a, b) => b.at - a.at)) {
    const folder = file.path.slice(0, Math.max(0, file.path.replace(/\\/g, "/").lastIndexOf("/")));
    const machine = grouped.get(file.machine) ?? new Map<string, FileSeen[]>();
    machine.set(folder, [...(machine.get(folder) ?? []), file]);
    grouped.set(file.machine, machine);
  }
  return grouped;
}

/** What wants a look: deeds that failed, were stopped or lost, ran long,
 *  said a great deal, or removed files. */
export type HeedKind = "failed" | "stopped" | "lost" | "long" | "big" | "removed";

export const LONG_MS = 20 * 60_000;
export const BIG_BYTES = 8 * 1024 * 1024;

export function heeds(deeds: Placed[], now: number): Map<HeedKind, Placed[]> {
  const found = new Map<HeedKind, Placed[]>();
  const add = (kind: HeedKind, deed: Placed) => found.set(kind, [...(found.get(kind) ?? []), deed]);
  for (const deed of deeds) {
    if (deed.outcome === "failed") add("failed", deed);
    if (deed.outcome === "stopped") add("stopped", deed);
    if (deed.outcome === "lost") add("lost", deed);
    if (lasted(deed, now) >= LONG_MS) add("long", deed);
    if (deed.output >= BIG_BYTES) add("big", deed);
    if (deed.touched?.some((touched) => touched.how === "removed")) add("removed", deed);
  }
  return found;
}

/** The first line of a script, as a deed is named in a list. */
export function headline(script: string): string {
  const lines = script.split("\n").map((line) => line.trim()).filter(Boolean);
  // `on MACHINE <<'RUN'` says where; the work is the line after it.
  const first = lines[0] ?? "";
  const heredoc = /^on\s+\S+\s+<<-?\s*['"]?\w+['"]?\s*$/.test(first);
  const line = heredoc ? (lines[1] ?? first) : first.replace(/^on\s+\S+\s+/, "");
  return line.length > 120 ? `${line.slice(0, 119)}…` : line;
}
