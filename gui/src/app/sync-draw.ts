// The sync status page's drawings (`sync-page.ts`): the two lines between
// this computer and the hub, one each way; how far behind a device stood
// through the span, small beside it in the list and large where it is
// opened, the offline stretches left blank, a small ring where it came back
// and a dot where it caught up; and the small ring of how many of the
// devices are caught up, for the way in. One ink: the pencil's.

import { escape } from "../conversation/prose";
import { circle, dashed, dot, outline, ruled, ruledPoints, seeded, type Point } from "../pen/shapes";
import type { LagPoint } from "../world/sync";
import type { SyncWords } from "./sync-words";

const f = (n: number) => (Math.round(n * 10) / 10).toString();
const hint = (text: string) => ` title="${escape(text)}"`;
const MINUTE = 60_000;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

/** Each device's ink, in the order the hub lists them: a device keeps its
 *  ink while others come and go before the sixth. */
export const INKS = ["var(--pen-clay)", "var(--pen-sky)", "var(--pen-sage)", "var(--pen-rose)", "var(--pen-graphite)"];
/** How far behind still counts as caught up: what a minute's sampling
 *  cannot tell from catching up as it goes. */
export const CAUGHT_UP = MINUTE;
/** The tops the scale may take, the first above the most drawn. */
const TOPS = [MINUTE, 5 * MINUTE, 15 * MINUTE, 30 * MINUTE, HOUR, 3 * HOUR, 6 * HOUR, 12 * HOUR, DAY, 3 * DAY, 7 * DAY, 30 * DAY];

export interface Line {
  machine: string;
  name: string;
  ink: string;
  points: LagPoint[];
}

export interface LinesInput {
  width: number;
  height: number;
  from: number;
  to: number;
  step: number;
  span: "day" | "week";
  lines: Line[];
  words: SyncWords;
  seed: string;
}

/** How far behind each device stood, through the span. */
export function lagLines(input: LinesInput): string {
  const { width, height, from, to, step, span, lines, words } = input;
  const left = 56;
  const right = 14;
  const top = 12;
  const bottom = 24;
  const w = Math.max(40, width - left - right);
  const h = Math.max(40, height - top - bottom);
  const most = Math.max(0, ...lines.flatMap((line) => line.points.filter((point) => point.online).map((point) => point.behind)));
  const ceiling = TOPS.find((value) => value >= most * 1.1) ?? most * 1.1;
  const x = (at: number) => left + ((Math.min(to, Math.max(from, at)) - from) / Math.max(1, to - from)) * w;
  const y = (behind: number) => top + h - (Math.min(ceiling, behind) / ceiling) * h;
  const random = seeded(`sync-lines:${input.seed}`);
  let grid = "";
  let labels = "";
  for (const share of [0, 0.5, 1]) {
    grid += ruled([left, y(ceiling * share)], [left + w, y(ceiling * share)], share === 0 ? 1 : 0.6, random);
    labels += `<text class="ledger-axis" x="${left - 6}" y="${f(y(ceiling * share) + 4)}" text-anchor="end">${escape(share === 0 ? "0" : words.duration(ceiling * share))}</text>`;
  }
  // The day by every sixth hour, the week by its days.
  if (span === "day") {
    const first = new Date(from);
    first.setMinutes(0, 0, 0);
    while (first.getHours() % 6 !== 0 || first.getTime() < from) first.setTime(first.getTime() + HOUR);
    for (let at = first.getTime(); at <= to; at += 6 * HOUR) {
      labels += `<text class="ledger-axis" x="${f(x(at))}" y="${height - 6}" text-anchor="middle">${escape(words.axisTime(new Date(at), span))}</text>`;
      grid += ruled([x(at), y(0) - 2], [x(at), y(0) + 3], 0.8, random);
    }
  } else {
    const midnight = new Date(from);
    midnight.setHours(0, 0, 0, 0);
    for (let at = midnight.getTime() + DAY; at <= to; at = new Date(new Date(at).setDate(new Date(at).getDate() + 1)).getTime()) {
      grid += ruled([x(at), y(0) - 2], [x(at), y(0) + 3], 0.8, random);
      if (at + DAY / 2 <= to) labels += `<text class="ledger-axis" x="${f(x(at + DAY / 2))}" y="${height - 6}" text-anchor="middle">${escape(words.axisTime(new Date(at), span))}</text>`;
    }
  }
  let ink = "";
  let marks = "";
  for (const line of lines) {
    const points = [...line.points].sort((a, b) => a.at - b.at);
    let run: Point[] = [];
    const flush = () => {
      if (run.length > 1) ink += `<path fill="${line.ink}" d="${outline(run, 2, random, { pressure: 0.15, taper: 0.01 })}"/>`;
      else if (run.length === 1) ink += `<path fill="${line.ink}" d="${dot(run[0]!, 1.6, random)}"/>`;
      run = [];
    };
    points.forEach((point, index) => {
      const before = points[index - 1];
      const joined = before && before.online && point.at - before.at <= step * 1.5;
      if (!point.online) {
        flush();
        return;
      }
      if (!joined) {
        flush();
        // Back after a stretch away: a small ring where it came back.
        if (before) marks += `<path fill="${line.ink}" d="${circle([x(point.at), y(point.behind)], 3.4, 1, random)}"${hint(`${line.name} ${words.reconnected}`)}/>`;
      }
      run.push([x(point.at), y(point.behind)]);
      // Caught up again: a dot where it did.
      if (joined && before.behind >= CAUGHT_UP && point.behind < CAUGHT_UP) {
        marks += `<path fill="${line.ink}" d="${dot([x(point.at), y(point.behind)], 3, random)}"${hint(`${line.name} ${words.caughtUp}`)}/>`;
      }
    });
    flush();
  }
  // A stretch's hover: where every device stood then.
  let hits = "";
  const slot = Math.max(2, (w / Math.max(1, to - from)) * step);
  const byLine = lines.map((line) => new Map(line.points.map((point) => [point.at - (point.at % step), point])));
  for (let at = from - (from % step); at < to; at += step) {
    const said = lines
      .map((line, index) => {
        const point = byLine[index]!.get(at);
        if (!point) return null;
        const state = !point.online ? words.offline : point.behind < CAUGHT_UP ? words.synced : words.behindBy(words.duration(point.behind));
        return `${line.name} ${state}`;
      })
      .filter((one): one is string => one !== null);
    if (!said.length) continue;
    hits += `<rect x="${f(x(at))}" y="${top}" width="${f(slot)}" height="${f(h)}" fill="transparent"${hint([words.at(new Date(at), span), ...said].join("\n"))}/>`;
  }
  return `<svg class="ledger-chart sync-lines" viewBox="0 0 ${f(width)} ${height}" width="${f(width)}" height="${height}" role="img" aria-label="${escape(words.lineTitle)}"><path class="ledger-rule" d="${grid}"/>${labels}<g filter="url(#pen-marker-lite)">${ink}${marks}</g>${hits}</svg>`;
}

/** A line between this computer and the hub, drawn a thousand units long
 *  and stretched to the room it has, with the head it points with: broken
 *  into dashes where the two are cut off from each other. */
export function wayLine(seed: string, cut: boolean): { line: string; head: string } {
  const random = seeded(`sync-way:${seed}`);
  const d = cut ? dashed([0, 5], [1000, 5], 1.3, random, 14, 12) : outline(ruledPoints([0, 5], [1000, 5], random), 1.3, random, { pressure: 0.2, taper: 0.01 });
  const head = outline([[1.5, 1.5], [8.6, 5]], 1.5, random, { pressure: 0.15, taper: 0.1 }) + outline([[1.5, 8.5], [8.6, 5]], 1.5, random, { pressure: 0.15, taper: 0.1 });
  return {
    line: `<svg class="sy-way-line" viewBox="0 0 1000 10" preserveAspectRatio="none" aria-hidden="true"><path d="${d}"/></svg>`,
    head: `<svg class="sy-way-head" viewBox="0 0 10 10" aria-hidden="true"><path d="${head}"/></svg>`,
  };
}

/** How far behind one device stood through the span, small: a faint line
 *  the length of the span, inked where it was online, the ink rising with
 *  how far behind it stood against `ceiling`; blank where it was offline. */
export function strip(points: LagPoint[], from: number, to: number, ceiling: number, seed: string, width = 168, height = 22): string {
  const random = seeded(`sync-strip:${seed}`);
  const base = height - 4;
  const rise = height - 8;
  const slots = 56;
  const span = Math.max(1, to - from);
  // The most it stood behind in each slot of the span it was online in.
  const most: (number | null)[] = new Array(slots).fill(null);
  for (const point of points) {
    if (!point.online || point.at < from || point.at > to) continue;
    const slot = Math.min(slots - 1, Math.floor(((point.at - from) / span) * slots));
    most[slot] = Math.max(most[slot] ?? 0, point.behind);
  }
  const x = (slot: number) => 1 + ((slot + 0.5) / slots) * (width - 2);
  const y = (behind: number) => base - Math.sqrt(Math.min(1, behind / Math.max(1, ceiling))) * rise;
  let ink = "";
  let run: Point[] = [];
  const flush = () => {
    if (run.length > 1) ink += outline(run, 1.6, random, { pressure: 0.15, taper: 0.02 });
    else if (run.length === 1) ink += dot(run[0]!, 1.2, random);
    run = [];
  };
  most.forEach((behind, slot) => {
    if (behind === null) flush();
    else run.push([x(slot), y(behind < CAUGHT_UP ? 0 : behind)]);
  });
  flush();
  return `<svg class="sy-strip" viewBox="0 0 ${width} ${height}" width="${width}" height="${height}" aria-hidden="true"><path class="sy-strip-base" d="${ruled([1, base], [width - 1, base], 0.8, random)}"/><path class="sy-strip-ink" d="${ink}"/></svg>`;
}

/** A small ring, drawn round as far as `synced` of `devices`: the way in,
 *  beside 「我」 and on it. */
export function syncRing(synced: number, devices: number, size = 16, seed = "sync-ring"): string {
  const random = seeded(seed);
  const center: Point = [size / 2, size / 2];
  const radius = size / 2 - 2;
  const share = devices > 0 ? Math.max(0, Math.min(1, synced / devices)) : 0;
  const whole = circle(center, radius, 1, random);
  let arc = "";
  if (share > 0) {
    const steps = Math.max(6, Math.round(24 * share));
    const points: Point[] = [];
    for (let index = 0; index <= steps; index += 1) {
      const angle = -Math.PI / 2 + (index / steps) * Math.PI * 2 * share;
      points.push([center[0] + Math.cos(angle) * radius, center[1] + Math.sin(angle) * radius]);
    }
    arc = `<path class="sync-ring-ink" d="${outline(points, 2.2, random, { pressure: 0.12, taper: 0.04 })}"/>`;
  }
  return `<svg class="sync-ring" width="${size}" height="${size}" viewBox="0 0 ${size} ${size}" aria-hidden="true"><path class="sync-ring-trace" d="${whole}"/>${arc}</svg>`;
}
