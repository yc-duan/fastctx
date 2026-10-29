// The activity page's drawings, in the usage page's pen (`../app/usage/draw.ts`):
// a day as lanes of felt strokes, one lane a machine; a year of days as
// cells inked by how much ran; the hours of the week as dots; a file's
// lines added and removed as two short strokes. Each is drawn at the width
// it is shown at, and the hover over each mark says what it stands for.

import { escape } from "../conversation/prose";
import { felt } from "../pen/felt";
import { blot, dashed, dot, outline, ruled, seeded, swatch } from "../pen/shapes";
import { DAY_MS, HOUR_MS, began, dayOf, ended, failing, headline, lasted, weekdayOf, type Placed } from "./model";
import type { ActivityWords } from "./words";

const f = (n: number) => (Math.round(n * 10) / 10).toString();
const hint = (text: string) => ` title="${escape(text)}"`;
const LEVELS = [0, 0.3, 0.52, 0.76, 1];

/** The ink a deed is drawn in: the accent only where it went wrong. */
function inkOf(deed: Placed): string {
  if (failing(deed.outcome)) return "var(--pen-clay)";
  if (deed.outcome === "stopped") return "var(--pen-trace)";
  return "var(--pen-graphite)";
}

/** What a deed's hover says. */
export function said(deed: Placed, now: number, words: ActivityWords, when: (at: number) => string): string {
  const time = deed.outcome === "running" ? words.running(words.duration(lasted(deed, now))) : words.ran(words.duration(lasted(deed, now)));
  const parts = [headline(deed.script), `${when(began(deed))} · ${words.outcome[deed.outcome]}${deed.code !== undefined && deed.code !== 0 ? ` (${deed.code})` : ""} · ${time}`];
  if (deed.went?.length) parts.push(words.went(deed.went.join(", ")));
  if (deed.touched?.length) parts.push(words.files(deed.touched.length));
  return parts.join("\n");
}

export interface Lane {
  machine: string;
  deeds: Placed[];
  away: boolean;
}

export interface TimelineInput {
  width: number;
  lanes: Lane[];
  /** The day shown, as its local midnight. */
  day: number;
  now: number;
  words: ActivityWords;
  when(at: number): string;
  seed: string;
}

/** The hours of `day` that held anything, a little round them, whole
 *  hours, three at least. */
function window(lanes: Lane[], day: number, now: number): [number, number] {
  const times = lanes.flatMap((lane) => lane.deeds.flatMap((deed) => [began(deed), ended(deed) ?? now]));
  const dayEnd = day + DAY_MS;
  if (!times.length) {
    const end = Math.min(dayEnd, Math.max(day + 3 * HOUR_MS, Math.ceil(now / HOUR_MS) * HOUR_MS));
    return [Math.max(day, end - 6 * HOUR_MS), end];
  }
  let from = Math.max(day, Math.floor((Math.min(...times) - HOUR_MS / 2) / HOUR_MS) * HOUR_MS);
  let to = Math.min(dayEnd, Math.ceil((Math.max(...times) + HOUR_MS / 2) / HOUR_MS) * HOUR_MS);
  while (to - from < 3 * HOUR_MS) {
    if (to < dayEnd) to += HOUR_MS;
    else if (from > day) from -= HOUR_MS;
    else break;
  }
  return [from, to];
}

/** Deeds that overlap go on tracks one under another, as many as needed,
 *  four at most; a fifth shares the last. */
function tracks(deeds: Placed[], now: number, minimum: number): Map<Placed, number> {
  const ends: number[] = [];
  const placed = new Map<Placed, number>();
  for (const deed of [...deeds].sort((a, b) => began(a) - began(b))) {
    const start = began(deed);
    const finish = Math.max(ended(deed) ?? now, start + minimum);
    let track = ends.findIndex((end) => end <= start);
    if (track < 0) track = Math.min(ends.length, 3);
    ends[track] = Math.max(ends[track] ?? 0, finish);
    placed.set(deed, track);
  }
  return placed;
}

/** A day as lanes, one a machine, each deed a felt stroke from its start
 *  to its end, the hours ruled across the top. */
export function timeline(input: TimelineInput): { svg: string; height: number } {
  const { width, lanes, day, now, words } = input;
  const narrow = width < 560;
  const left = narrow ? 76 : 120;
  const top = 26;
  const right = 10;
  const [from, to] = window(lanes, day, now);
  const span = to - from;
  const x = (at: number) => left + ((Math.min(Math.max(at, from), to) - from) / span) * (width - left - right);
  const random = seeded(`timeline:${input.seed}`);
  // A stroke is never shorter than a dab: this much of the day.
  const minimum = (6 / (width - left - right)) * span;
  let y = top;
  let rules = "";
  let ink = "";
  let hits = "";
  let labels = "";
  const stroke = 9;
  const pitch = 13;
  for (const lane of lanes) {
    const placed = tracks(lane.deeds, now, minimum);
    const count = Math.max(1, ...[...placed.values()].map((track) => track + 1));
    const height = Math.max(30, 14 + count * pitch);
    const name = lane.machine.length > (narrow ? 9 : 15) ? `${lane.machine.slice(0, narrow ? 8 : 14)}…` : lane.machine;
    labels += `<text class="ay-lane-name${lane.away ? " away" : ""}" x="0" y="${f(y + height / 2 + 4)}"${lane.machine !== name ? hint(lane.machine) : ""}>${escape(name)}</text>`;
    if (lane.away) labels += `<text class="ledger-axis" x="${f(left)}" y="${f(y + height / 2 + 4)}">${escape(words.away)}</text>`;
    for (const [deed, track] of placed) {
      const start = x(began(deed));
      const end = Math.max(start + 6, x(ended(deed) ?? now));
      const cy = y + 9 + track * pitch + stroke / 2;
      ink += `<path fill="${inkOf(deed)}" fill-opacity="${deed.outcome === "running" ? 0.55 : 0.88}" d="${felt(start, end, cy, stroke, random)}"/>`;
      if (deed.outcome === "running") ink += `<path fill="var(--pen-graphite)" d="${blot([end + 3, cy], 1.8, random)}"/>`;
      hits += `<rect class="ledger-hit" data-job="${escape(deed.job)}" data-machine="${escape(deed.machine)}" x="${f(start - 2)}" y="${f(cy - pitch / 2)}" width="${f(end - start + 4)}" height="${pitch}" rx="3"${hint(said(deed, now, words, input.when))}/>`;
    }
    y += height;
    rules += `<path class="ledger-rule faint" d="${ruled([left, y], [width - right, y], 0.6, random)}"/>`;
  }
  // The hours along the top, as many as fit.
  const hours = span / HOUR_MS;
  const every = [1, 2, 3, 4, 6].find((step) => ((width - left - right) / hours) * step >= (narrow ? 34 : 44)) ?? 6;
  let axis = "";
  for (let at = from; at <= to; at += HOUR_MS) {
    const hour = new Date(at).getHours();
    if (hour % every) continue;
    const ax = x(at);
    // A label too near the right edge would be cut: its tick says enough.
    if (ax <= width - right - 16) axis += `<text class="ledger-axis" x="${f(ax)}" y="${top - 10}" text-anchor="middle">${hour}:00</text>`;
    axis += `<path class="ledger-rule faint" d="${ruled([ax, top - 4], [ax, top], 0.6, random)}"/>`;
  }
  rules += `<path class="ledger-rule" d="${ruled([left, top], [width - right, top], 0.9, random)}"/>`;
  if (now > from && now < to) {
    rules += `<path class="ay-now-line" d="${dashed([x(now), top + 2], [x(now), y], 1, random, 3, 4)}"/>`;
  }
  const height = Math.max(y + 6, top + 40);
  return {
    svg: `<svg class="ledger-chart ay-timeline" viewBox="0 0 ${f(width)} ${f(height)}" width="${f(width)}" height="${f(height)}">${rules}${axis}${labels}<g class="ledger-ink" filter="url(#pen-felt)">${ink}</g>${hits}</svg>`,
    height,
  };
}

export interface YearInput {
  width: number;
  /** The room it may take: the weeks' bars take what the days leave. */
  height: number;
  days: Map<number, { runs: number; failed: number }>;
  today: number;
  words: ActivityWords;
  /** A day as the hover names it. */
  date(day: number): string;
  month(month: number): string;
  seed: string;
}

/** A year of days, a column a week from Monday down, each day inked by how
 *  much ran, by quarters of the days that ran anything; under each column
 *  that week's runs as a felt stroke, so the year's trend reads straight
 *  down from the days it sums. As many recent weeks as fit. */
export function year(input: YearInput): { svg: string; height: number } {
  const { width, days, today, words } = input;
  const left = 22;
  const top = 18;
  const weeks = Math.max(8, Math.min(53, Math.floor((width - left - 4) / 13)));
  const pitch = Math.min(19, (width - left - 4) / weeks);
  const cell = pitch - 3;
  const first = today - (weekdayOf(today) + (weeks - 1) * 7) * DAY_MS;
  const ran = [...days.values()].map((day) => day.runs).filter((runs) => runs > 0).sort((a, b) => a - b);
  const quarter = (share: number) => ran[Math.min(ran.length - 1, Math.floor(share * ran.length))] ?? 1;
  const cuts = [quarter(0.25), quarter(0.5), quarter(0.75)];
  const levelOf = (runs: number) => (runs <= 0 ? 0 : 1 + cuts.filter((cut) => runs > cut).length);
  const random = seeded(`year:${input.seed}`);
  let empty = "";
  let inked = "";
  let hits = "";
  const sums: number[] = [];
  let lastMonth = -1;
  const named: { x: number; month: number }[] = [];
  for (let week = 0; week < weeks; week += 1) {
    for (let weekday = 0; weekday < 7; weekday += 1) {
      const day = dayOf(first + (week * 7 + weekday) * DAY_MS + HOUR_MS * 12);
      if (day > today) continue;
      const seen = days.get(day);
      const cx = left + week * pitch;
      const cy = top + weekday * pitch;
      const path = swatch(cx, cy, cell, cell, random);
      const level = levelOf(seen?.runs ?? 0);
      sums[week] = (sums[week] ?? 0) + (seen?.runs ?? 0);
      if (level) inked += `<path d="${path}" fill="var(--pen-graphite)" fill-opacity="${LEVELS[level]}"/>`;
      else empty += path;
      hits += `<rect class="ledger-hit" data-day="${day}" x="${f(cx - 1)}" y="${f(cy - 1)}" width="${f(pitch)}" height="${f(pitch)}" rx="2"${hint(words.day(input.date(day), seen?.runs ?? 0, seen?.failed ?? 0))}/>`;
      if (day === today) hits += `<path class="ledger-today" d="${blot([cx + cell / 2, cy + cell + 3.6], 1.4, random)}"/>`;
      if (weekday === 0) {
        const month = new Date(day).getMonth();
        if (month !== lastMonth && week < weeks - 1) named.push({ x: cx, month });
        lastMonth = month;
      }
    }
  }
  const months = named
    .filter((one, index) => index === named.length - 1 || named[index + 1]!.x - one.x >= 32)
    .map((one) => `<text class="ledger-axis" x="${f(one.x)}" y="${top - 6}">${escape(input.month(one.month))}</text>`)
    .join("");
  const labels = [0, 2, 4, 6].map((weekday) => `<text class="ledger-axis" x="0" y="${f(top + weekday * pitch + cell - 1)}">${escape(words.weekday[weekday]!)}</text>`).join("");
  // Each week's runs, a stroke up from a ruled base under its column.
  const barTop = top + 7 * pitch + 18;
  const barHeight = Math.max(60, Math.min(160, input.height - barTop - 10));
  const most = Math.max(1, ...sums.map((sum) => sum ?? 0));
  let bars = "";
  let columns = "";
  sums.forEach((sum, week) => {
    const x = left + week * pitch;
    if (sum) {
      const height = Math.max(2, (sum / most) * barHeight);
      bars += `<path fill="var(--pen-graphite)" fill-opacity="0.5" d="${swatch(x, barTop + barHeight - height, cell, height, random, 1.6)}"/>`;
    }
    const said = `${input.date(first + week * 7 * DAY_MS + HOUR_MS * 12)}\n${words.count(sum ?? 0)}`;
    columns += `<rect class="ledger-hit" x="${f(x - 1.5)}" y="${f(barTop - 4)}" width="${f(pitch)}" height="${f(barHeight + 6)}" rx="3"${hint(said)}/>`;
  });
  const base = `<path class="ledger-rule" d="${ruled([left, barTop + barHeight + 1], [left + weeks * pitch - 3, barTop + barHeight + 1], 0.9, random)}"/>`;
  const height = barTop + barHeight + 8;
  return {
    svg: `<svg class="ledger-chart ay-year" viewBox="0 0 ${f(width)} ${height}" width="${f(width)}" height="${height}"><path class="ledger-empty" d="${empty}"/>${columns}<g class="ledger-ink" filter="url(#pen-marker-lite)">${inked}${bars}</g>${base}${months}${labels}${hits}</svg>`,
    height,
  };
}

/** The hours of the week as dots, each as large as what ran in it. */
export function rhythm(width: number, height: number, grid: number[][], words: ActivityWords, seed: string): string {
  const left = 30;
  const top = 6;
  const bottom = 22;
  const cw = (width - left - 6) / 24;
  const ch = (height - top - bottom) / 7;
  const largest = Math.min(cw, ch) * 0.46;
  const most = Math.max(1, ...grid.flat());
  const random = seeded(`rhythm:${seed}`);
  let base = "";
  let dots = "";
  let labels = "";
  let hits = "";
  for (let weekday = 0; weekday < 7; weekday += 1) {
    labels += `<text class="ledger-axis" x="${left - 8}" y="${f(top + ch * weekday + ch / 2 + 4)}" text-anchor="end">${escape(words.weekday[weekday]!)}</text>`;
    for (let hour = 0; hour < 24; hour += 1) {
      const cx = left + cw * hour + cw / 2;
      const cy = top + ch * weekday + ch / 2;
      const runs = grid[weekday]![hour]!;
      base += dot([cx, cy], 1.1, random);
      if (runs) dots += dot([cx, cy], Math.max(2, Math.sqrt(runs / most) * largest), random);
      hits += `<rect x="${f(cx - cw / 2)}" y="${f(cy - ch / 2)}" width="${f(cw)}" height="${f(ch)}" fill="transparent"${hint(words.hourOfWeek(words.weekday[weekday]!, hour, runs))}/>`;
    }
  }
  const marks = width < 520 ? [0, 6, 12, 18] : [0, 3, 6, 9, 12, 15, 18, 21];
  for (const hour of marks) labels += `<text class="ledger-axis" x="${f(left + cw * hour + cw / 2)}" y="${height - 6}" text-anchor="middle">${hour}:00</text>`;
  return `<svg class="ledger-chart ay-rhythm" viewBox="0 0 ${f(width)} ${height}" width="${f(width)}" height="${height}"><path class="ledger-trace" d="${base}"/><g filter="url(#pen-marker-lite)"><path fill="var(--pen-graphite)" fill-opacity="0.88" d="${dots}"/></g>${labels}${hits}</svg>`;
}

/** A session's runs along the span shown, a short upright stroke each on a
 *  ruled line, one that went wrong in clay. */
export function runs(times: { at: number; failed: boolean }[], from: number, to: number, seed: string): string {
  const width = 360;
  const random = seeded(`runs:${seed}`);
  const x = (at: number) => 3 + ((Math.min(Math.max(at, from), to) - from) / Math.max(1, to - from)) * (width - 6);
  let ink = "";
  let wrong = "";
  for (const time of times) {
    const stroke = outline([[x(time.at), 3], [x(time.at), 13]], 1.6, random, { pressure: 0.2, taper: 0.2 });
    if (time.failed) wrong += stroke;
    else ink += stroke;
  }
  return `<svg class="ay-runs" viewBox="0 0 ${width} 16" preserveAspectRatio="none" aria-hidden="true"><path class="ledger-trace" d="${ruled([2, 8], [width - 2, 8], 0.6, random)}"/><path fill="var(--pen-graphite)" fill-opacity="0.75" d="${ink}"/><path fill="var(--pen-clay)" d="${wrong}"/></svg>`;
}

/** A file's lines added and removed, as two short strokes against the
 *  longest of its list. */
export function lines(added: number | undefined, removed: number | undefined, most: number, seed: string): string {
  if (added === undefined && removed === undefined) return "";
  const random = seeded(`lines:${seed}`);
  const width = 64;
  const scale = (n: number) => (n ? Math.max(3, (Math.sqrt(n) / Math.sqrt(Math.max(1, most))) * (width / 2 - 4)) : 0);
  const a = scale(added ?? 0);
  const r = scale(removed ?? 0);
  const ink =
    (a ? `<path fill="var(--add-fg)" d="${felt(2, 2 + a, 7, 6, random)}"/>` : "") +
    (r ? `<path fill="var(--del-fg)" d="${felt(width / 2 + 2, width / 2 + 2 + r, 7, 6, random)}"/>` : "");
  return `<svg class="ay-lines" viewBox="0 0 ${width} 14" width="${width}" height="14" aria-hidden="true"><g filter="url(#pen-felt)">${ink}</g></svg>`;
}

/** How long a deed has run, as a felt stroke that grows slower the longer
 *  it goes: a minute is a short dab, an hour most of the way. */
export function elapsed(ms: number, seed: string, width = 120): string {
  const random = seeded(`elapsed:${seed}`);
  const share = Math.min(1, Math.log10(1 + ms / 10_000) / Math.log10(1 + 7_200_000 / 10_000));
  const length = Math.max(6, share * (width - 6));
  return `<svg class="ay-elapsed" viewBox="0 0 ${width} 12" width="${width}" height="12" aria-hidden="true"><path class="ledger-trace" d="${ruled([2, 6], [width - 2, 6], 0.6, random)}"/><g filter="url(#pen-felt)"><path fill="var(--pen-graphite)" fill-opacity="0.7" d="${felt(2, 2 + length, 6, 7, random)}"/></g></svg>`;
}
