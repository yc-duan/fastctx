// The drawings of a login's usage, in the interface's pen: rings for its
// windows, a year of days, the week's limit as a line through time, the
// reset weeks as stacked bars, the hours of the week as dots, and what the
// use went to as one bar of parts. Each is drawn at the width it is shown at,
// so its labels keep their size on a phone; the hover over each mark says
// what it stands for.

import { escape } from "../../conversation/prose";
import { felt } from "../../pen/felt";
import { blot, dashed, dot, outline, ruled, seeded, swatch, type Point } from "../../pen/shapes";
import { DAY_MS, HOUR_MS, dayOf, daysBefore, levels, total, weekdayOf, type Person, type Track } from "./model";
import type { UsageWords } from "./words";

const f = (n: number) => (Math.round(n * 10) / 10).toString();
const hint = (text: string) => ` title="${escape(text)}"`;
/** The inks of the calendar's four levels, as shares of the full ink. */
const LEVELS = [0, 0.34, 0.56, 0.78, 1];

/** How an amount is written in a label and in a hover. */
export interface Amounts {
  /** In a hover or a figure: "约 $3.20", "1.2M". */
  say(value: number): string;
  /** On an axis: "$40", "2M". */
  axis(value: number): string;
}

/** Each person's part of an amount, for a hover: "我 约 $3.20 ada 约 $1.10". */
function split(values: Map<number, number> | undefined, people: Person[], amounts: Amounts): string {
  if (!values || people.length < 2) return "";
  const parts = people.filter((person) => (values.get(person.index) ?? 0) > 0).map((person) => `${person.name} ${amounts.say(values.get(person.index)!)}`);
  return parts.length > 1 ? `\n${parts.join("\n")}` : "";
}

/** One person's part of a ring, in their ink; a faded part is someone's
 *  other than the one looked at. */
export interface RingPart {
  share: number;
  ink: string;
  faded?: boolean;
}

/** How far one stroke of a bar laid after another runs back over its
 *  tail, as a share of the stroke's height; and the shortest part that is
 *  still a stroke of the marker, in pixels. */
const OVERLAP = 0.4;
const SHORTEST = 2.5;

/** What an ink drawing is of, kept on it so `fitInks` can draw it again at
 *  the width it is shown at: a felt stroke is drawn at its own size, never
 *  stretched, or its ends would squash. */
type Inked =
  | { kind: "meter"; share: number | null; seed: string; parts: RingPart[]; limit: boolean; height: number }
  | { kind: "bar"; share: number; seed: string; height: number; ink: string }
  | { kind: "parts"; shares: { value: number; ink: string; said: string }[]; seed: string };

function marked(svg: string, inked: Inked, width: number): string {
  return svg.replace("<svg ", `<svg data-ink="${escape(JSON.stringify(inked))}" data-width="${Math.round(width)}" `);
}

/** Draws again, at the width each is shown at, every ink drawing in `root`
 *  that is drawn at another. */
export function fitInks(root: ParentNode): void {
  // All strokes have their width from the surrounding layout. Read those
  // widths together before replacing any SVG and invalidating that layout.
  const drawings = Array.from(root.querySelectorAll<SVGSVGElement>("svg[data-ink]"), (svg) => ({
    svg, width: Math.round(svg.getBoundingClientRect().width),
  }));
  for (const { svg, width } of drawings) {
    if (width < 8 || Number(svg.dataset.width) === width) continue;
    const inked = JSON.parse(svg.dataset.ink!) as Inked;
    svg.outerHTML =
      inked.kind === "meter"
        ? meter(inked.share, inked.seed, inked.parts, inked.limit, width, inked.height)
        : inked.kind === "bar"
          ? bar(inked.share, inked.seed, width, inked.height, inked.ink)
          : parts(inked.shares, inked.seed, width);
  }
}

/** Felt strokes laid end to end from `x0` over `length`, one for each of
 *  `shares` (a share of the whole length) in its ink: each after the first
 *  runs back a little over the tail of the one before, the later ink laid
 *  over the earlier and the earlier showing through its grain; one too
 *  short to be a stroke is left out, its figure said beside the bar. */
function strokes(shares: { share: number; fill?: string; className?: string }[], x0: number, length: number, y: number, height: number, random: () => number): string {
  let x = x0;
  let ink = "";
  let before = 0;
  for (const one of shares) {
    const span = length * one.share;
    const end = x + span;
    if (span >= SHORTEST) {
      const back = before ? Math.min(height * OVERLAP, before * 0.3, span * 0.4) : 0;
      // A class gives its own ink.
      const fill = one.className ? ` class="${one.className}"` : ` style="fill:${one.fill}"`;
      ink += `<path${fill} d="${felt(x - back, end, y, height, random)}"/>`;
      before = span;
    }
    x = end;
  }
  return ink;
}

/** A limit's bar: a ruled line, inked along how much of it is used in one
 *  stroke of a broad felt marker, clay turning to caution near the end and
 *  to wrong past it. Given the parts of those who used it, each is its own
 *  stroke in its own colour, laid end to end, and what no part covers is
 *  graphite. What is no limit (`limit` false) keeps its inks to the end. */
export function meter(share: number | null, seed: string, parts: RingPart[] = [], limit = true, width = 400, height = 14): string {
  const random = seeded(`meter:${seed}`);
  const y = height / 2;
  const whole = ruled([2, y], [width - 2, y], 1, random);
  const level = share === null || !limit ? "" : share >= 1 ? " spent" : share >= 0.9 ? " high" : "";
  const thick = height * 0.72;
  let ink = "";
  if (share !== null && share >= 0.005) {
    const used = Math.min(1, share);
    const shown = parts.filter((part) => part.share >= 0.003);
    // Near the end, the warning says more than who used it.
    if (!shown.length || level) ink = strokes([{ share: used, className: `ledger-arc${level}` }], 3, width - 6, y, thick, random);
    else {
      // Faded inks are solid mixes with the page, not transparent ones:
      // the marker's grain turns a half-transparent ink to speckle.
      const laid: { share: number; fill: string }[] = [];
      let from = 0;
      for (const part of shown) {
        const to = Math.min(used, from + part.share);
        laid.push({ share: to - from, fill: part.faded ? `color-mix(in srgb, ${part.ink} 34%, var(--bg))` : part.ink });
        from = to;
      }
      if (used - from >= 0.003) laid.push({ share: used - from, fill: "color-mix(in srgb, var(--pen-graphite) 55%, var(--bg))" });
      ink = strokes(laid, 3, width - 6, y, thick, random);
    }
  }
  return marked(
    `<svg class="ledger-meter-ink${share === null ? " unread" : ""}" viewBox="0 0 ${width} ${height}" preserveAspectRatio="none" aria-hidden="true"><path class="ledger-trace" d="${whole}"/><g filter="url(#pen-felt)">${ink}</g></svg>`,
    { kind: "meter", share, seed, parts, limit, height },
    width,
  );
}

/** A short bar: a ruled line, inked along `share` of it in one stroke. */
export function bar(share: number, seed: string, width: number, height: number, ink = "var(--pen-clay)"): string {
  const random = seeded(`bar:${seed}`);
  const y = height / 2;
  const whole = ruled([2, y], [width - 2, y], 1, random);
  const used = Math.min(1, share);
  const filled = share > 0.005 ? strokes([{ share: Math.max(used, SHORTEST / Math.max(1, width - 6)), fill: ink }], 3, width - 6, y, height * 0.72, random) : "";
  return marked(
    `<svg class="ledger-bar" viewBox="0 0 ${width} ${height}" preserveAspectRatio="none" aria-hidden="true"><path class="ledger-trace" d="${whole}"/><g filter="url(#pen-felt)">${filled}</g></svg>`,
    { kind: "bar", share, seed, height, ink },
    width,
  );
}

export interface CalendarInput {
  width: number;
  /** The height it may take: what the year leaves, the weeks' bars take. */
  height?: number;
  days: Map<number, Map<number, number>>;
  /** The people shown, for each week's stacked total and the hovers. */
  people: Person[];
  today: number;
  ink: string;
  amounts: Amounts;
  words: UsageWords;
  seed: string;
}

/** A year of days, a column a week from Monday down, each day inked by
 *  how much it took against the others; under each column that week's
 *  total, stacked by person, so the year's trend reads straight down
 *  from the days it sums. A narrow screen shows as many recent weeks as
 *  fit. */
export function calendar(input: CalendarInput): { svg: string; height: number; weeks: number } {
  const { width, days, people, today, ink, amounts, words } = input;
  const left = 30;
  const top = 18;
  const weeks = Math.max(8, Math.min(53, Math.floor((width - left - 4) / 11)));
  const pitch = Math.min(15, (width - left - 4) / weeks);
  const cell = pitch - 3;
  const first = daysBefore(today, weekdayOf(today) + (weeks - 1) * 7);
  const dayAt = (week: number, weekday: number) => daysBefore(first, -(week * 7 + weekday));
  const level = levels([...days.values()].map(total));
  const random = seeded(`calendar:${input.seed}`);
  let empty = "";
  let inked = "";
  let hits = "";
  let lastMonth = -1;
  const named: { x: number; month: number }[] = [];
  for (let week = 0; week < weeks; week += 1) {
    for (let weekday = 0; weekday < 7; weekday += 1) {
      const day = dayAt(week, weekday);
      if (day > today) continue;
      const values = days.get(day);
      const sum = total(values);
      const x = left + week * pitch;
      const y = top + weekday * pitch;
      const path = swatch(x, y, cell, cell, random);
      const shade = level(sum);
      if (shade) inked += `<path d="${path}" fill="${ink}" fill-opacity="${LEVELS[shade]}"/>`;
      else empty += path;
      const said = `${words.day(new Date(day))}\n${sum ? amounts.say(sum) : words.unused}${split(values, people, amounts)}`;
      hits += `<rect x="${f(x - 1)}" y="${f(y - 1)}" width="${f(pitch)}" height="${f(pitch)}" fill="transparent"${hint(said)}/>`;
      if (day === today) hits += `<path class="ledger-today" d="${blot([x + cell / 2, y + cell + 3.6], 1.4, random)}"/>`;
      if (weekday === 0) {
        const month = new Date(day).getMonth();
        if (month !== lastMonth && week < weeks - 1) named.push({ x, month });
        lastMonth = month;
      }
    }
  }
  // A month named over the week it begins in, unless the next begins too
  // soon after for both names to fit: the later one is kept.
  const months = named
    .filter((one, index) => index === named.length - 1 || named[index + 1]!.x - one.x >= 32)
    .map((one) => `<text class="ledger-axis" x="${f(one.x)}" y="${top - 6}">${escape(words.month(one.month))}</text>`)
    .join("");
  const labels = [0, 2, 4, 6].map((weekday) => `<text class="ledger-axis" x="0" y="${f(top + weekday * pitch + cell - 1)}">${escape(words.weekday[weekday]!.slice(-1))}</text>`).join("");
  // Each week's total, stacked by person.
  const barTop = top + 7 * pitch + 12;
  const barHeight = Math.max(44, (input.height ?? 0) - barTop - 6);
  const sums = Array.from({ length: weeks }, (_, week) => {
    const byPerson = new Map<number, number>();
    for (let weekday = 0; weekday < 7; weekday += 1) for (const [person, value] of days.get(dayAt(week, weekday)) ?? []) byPerson.set(person, (byPerson.get(person) ?? 0) + value);
    return byPerson;
  });
  const scale = nice(Math.max(1e-9, ...sums.map(total)));
  // The weeks' own head room: their scale's figure and the people's inks.
  const bandTop = barTop + 16;
  const bandHeight = barTop + barHeight - bandTop;
  const yOf = (value: number) => bandTop + bandHeight - (value / scale) * bandHeight;
  let bars = "";
  let columns = "";
  sums.forEach((byPerson, week) => {
    const x = left + week * pitch;
    let below = 0;
    for (const person of people) {
      const value = byPerson.get(person.index) ?? 0;
      if (!value) continue;
      const y0 = yOf(below);
      const y1 = yOf(below + value);
      below += value;
      bars += `<path fill="${person.ink}" fill-opacity="0.85" d="${swatch(x, y1, cell, Math.max(1.2, y0 - y1), random, 1.6)}"/>`;
    }
    const sum = total(byPerson);
    const said = `${words.weekFrom(new Date(dayAt(week, 0)))}\n${sum ? amounts.say(sum) : words.unused}${split(byPerson, people, amounts)}`;
    columns += `<rect class="ledger-hit" x="${f(x - 1.5)}" y="${f(bandTop - 4)}" width="${f(pitch)}" height="${f(bandHeight + 6)}" rx="3"${hint(said)}/>`;
  });
  const keyed = people.length > 1 ? peopleKeys(people, left + weeks * pitch - 3, barTop + 8) : "";
  const base =
    `<path class="ledger-rule" d="${ruled([left, barTop + barHeight + 1], [left + weeks * pitch - 3, barTop + barHeight + 1], 0.9, random)}"/>` +
    `<path class="ledger-rule faint" d="${ruled([left, bandTop], [left + weeks * pitch - 3, bandTop], 0.6, random)}"/>` +
    `<text class="ledger-axis" x="0" y="${f(bandTop + 4)}">${escape(amounts.axis(scale))}</text>` +
    `<text class="ledger-axis" x="0" y="${barTop + barHeight - 2}">${escape(words.weekRow)}</text>`;
  const height = barTop + barHeight + 6;
  return {
    svg: `<svg class="ledger-chart" viewBox="0 0 ${f(width)} ${height}" width="${f(width)}" height="${height}"><path class="ledger-empty" d="${empty}"/>${columns}<g class="ledger-ink" filter="url(#pen-marker-lite)">${inked}${bars}</g>${base}${keyed}${months}${labels}${hits}</svg>`,
    height,
    weeks,
  };
}

/** The last seventeen weeks as a small calendar, for a login's row. */
export function miniCalendar(days: Map<number, Map<number, number>>, today: number, seed: string, ink = "var(--pen-clay)"): string {
  const weeks = 17;
  const pitch = 9;
  const cell = 7;
  const first = daysBefore(today, weekdayOf(today) + (weeks - 1) * 7);
  const level = levels([...days.values()].map(total));
  const random = seeded(`mini:${seed}`);
  let empty = "";
  let inked = "";
  for (let week = 0; week < weeks; week += 1) {
    for (let weekday = 0; weekday < 7; weekday += 1) {
      const day = daysBefore(first, -(week * 7 + weekday));
      if (day > today) continue;
      const path = swatch(week * pitch + 1, weekday * pitch + 1, cell, cell, random, 1.6);
      const shade = level(total(days.get(day)));
      if (shade) inked += `<path d="${path}" fill="${ink}" fill-opacity="${LEVELS[shade]}"/>`;
      else empty += path;
    }
  }
  return `<svg class="ledger-mini" viewBox="0 0 ${weeks * pitch} ${7 * pitch}" width="${weeks * pitch}" height="${7 * pitch}" aria-hidden="true"><path class="ledger-empty" d="${empty}"/><g filter="url(#pen-marker-lite)">${inked}</g></svg>`;
}

export interface QuotaInput {
  width: number;
  height: number;
  start: number;
  end: number;
  now: number;
  /** The login's own week, which begins at none used. */
  window: boolean;
  week: Track | null;
  five: Track | null;
  /** How fast the week fills, as a share an hour. */
  pace: number | null;
  words: UsageWords;
  seed: string;
  /** What the hover over an hour says, where it is not the windows'. */
  at?: (at: Date, week: number | null) => string;
  /** The week's ink, where it is someone's own. */
  ink?: string;
}

/** The week's limit through the week: the seven-day share as a line with
 *  the area under it, carried on at the last day's pace to the reset or
 *  to full; the five-hour windows, where there are any, as a lower band of
 *  their own. */
export function quota(input: QuotaInput): { svg: string; said: string } {
  const { width, height, start, end, now, week, five, pace, words } = input;
  const left = 40;
  const right = 12;
  const top = 12;
  const bottom = 22;
  const w = width - left - right;
  const band = five ? 38 : 0;
  const main = height - top - bottom - band - (five ? 16 : 0);
  const x = (at: number) => left + ((Math.min(end, Math.max(start, at)) - start) / Math.max(1, end - start)) * w;
  const y = (share: number) => top + main - Math.min(1.04, share) * main;
  const bandTop = top + main + 16;
  const yb = (share: number) => bandTop + band - Math.min(1, share) * band;
  const random = seeded(`quota:${input.seed}`);
  let grid = "";
  let labels = "";
  for (const share of [0, 0.25, 0.5, 0.75, 1]) {
    grid += ruled([left, y(share)], [left + w, y(share)], share === 0 ? 1 : 0.6, random);
    labels += `<text class="ledger-axis" x="${left - 6}" y="${f(y(share) + 4)}" text-anchor="end">${share * 100}%</text>`;
  }
  if (five) {
    grid += ruled([left, yb(0)], [left + w, yb(0)], 0.9, random);
    labels += `<text class="ledger-axis" x="${left - 6}" y="${f(yb(0) - 2)}" text-anchor="end">5h</text>`;
  }
  // Each whole day named under its middle, each midnight ticked.
  for (let day = daysBefore(dayOf(start), -1); day < end; day = daysBefore(day, -1)) {
    if (day + DAY_MS / 2 < end) labels += `<text class="ledger-axis" x="${f(x(day + DAY_MS / 2))}" y="${height - 6}" text-anchor="middle">${escape(words.weekday[weekdayOf(day)]!)}</text>`;
    grid += ruled([x(day), yb(0) - 2], [x(day), yb(0) + 3], 0.8, random);
  }
  const shown = (track: Track | null, from: boolean): [number, number][] => {
    if (!track) return [];
    const points = track.points.filter(([at]) => at >= start && at <= Math.max(now, end));
    return from && points.length && points[0]![0] > start ? [[start, 0], ...points] : points;
  };
  const weekPoints = shown(week, input.window);
  const fivePoints = shown(five, false);
  // A share holds until the next reading: drawn as steps, not slopes.
  const stepped = (points: [number, number][], until: number): [number, number][] => {
    const out: [number, number][] = [];
    points.forEach(([at, share], index) => {
      if (index) out.push([at, points[index - 1]![1]]);
      out.push([at, share]);
    });
    if (points.length) out.push([until, points[points.length - 1]![1]]);
    return out;
  };
  const lastAt = Math.min(now, end);
  let ink = "";
  let areas = "";
  if (weekPoints.length) {
    const steps = stepped(weekPoints, lastAt);
    areas += `<path class="ledger-area"${input.ink ? ` style="fill:color-mix(in srgb, ${input.ink} 14%, transparent)"` : ""} d="M${f(x(steps[0]![0]))} ${f(y(0))}L${steps.map(([at, share]) => `${f(x(at))} ${f(y(share))}`).join("L")}L${f(x(lastAt))} ${f(y(0))}Z"/>`;
    ink += `<path fill="${input.ink ?? "var(--pen-clay)"}" d="${outline(steps.map(([at, share]) => [x(at), y(share)] as Point), 2.2, random, { pressure: 0.15, taper: 0.01 })}"/>`;
  }
  if (fivePoints.length) {
    const steps = stepped(fivePoints, lastAt);
    areas += `<path class="ledger-area-five" d="M${f(x(steps[0]![0]))} ${f(yb(0))}L${steps.map(([at, share]) => `${f(x(at))} ${f(yb(share))}`).join("L")}L${f(x(lastAt))} ${f(yb(0))}Z"/>`;
    ink += `<path fill="var(--pen-graphite)" d="${outline(steps.map(([at, share]) => [x(at), yb(share)] as Point), 1.1, random, { pressure: 0.15, taper: 0.01 })}"/>`;
  }
  // Carried on at the last day's pace.
  let said = "";
  let carried = "";
  if (week && pace && now < end) {
    const fullAt = now + ((1 - week.last) / pace) * HOUR_MS;
    const until = Math.min(end, fullAt);
    const reaches = week.last + (pace * (until - now)) / HOUR_MS;
    said = fullAt <= end ? words.fullBy(new Date(fullAt)) : words.reachBy(Math.round(reaches * 100));
    const count = Math.max(6, Math.round((x(until) - x(now)) / 9));
    for (let index = 1; index <= count; index += 1) {
      const t = index / count;
      carried += dot([x(now + (until - now) * t), y(week.last + (reaches - week.last) * t)], 1.3, random);
    }
  }
  const nowLine = now < end ? `<path class="ledger-now-line" d="${ruled([x(now), top - 2], [x(now), yb(0)], 0.8, random)}"/><text class="ledger-axis strong" x="${f(x(now) + 5)}" y="${top + 9}">${escape(words.now)}</text>` : "";
  const resetMark = `<text class="ledger-axis" x="${f(left + w)}" y="${top - 2}" text-anchor="end">${escape(words.reset)}</text>`;
  // An hour's hover: the last shares read by then.
  let hits = "";
  const lastOf = (points: [number, number][], at: number) => {
    let found: number | null = null;
    for (const [when, share] of points) if (when <= at) found = share;
    return found;
  };
  const slot = Math.max(4, (w / Math.max(1, end - start)) * HOUR_MS);
  for (let at = start; at < lastAt; at += HOUR_MS) {
    const weekShare = lastOf(weekPoints, at + HOUR_MS);
    const fiveShare = lastOf(fivePoints, at + HOUR_MS);
    if (weekShare === null && fiveShare === null) continue;
    const pct = (share: number | null) => (share === null ? null : Math.round(share * 100));
    const said = input.at ? input.at(new Date(at + HOUR_MS), pct(weekShare)) : words.quotaAt(new Date(at + HOUR_MS), pct(weekShare), pct(fiveShare));
    hits += `<rect x="${f(x(at))}" y="${top}" width="${f(slot)}" height="${f(yb(0) - top)}" fill="transparent"${hint(said)}/>`;
  }
  return {
    svg: `<svg class="ledger-chart" viewBox="0 0 ${f(width)} ${height}" width="${f(width)}" height="${height}"><path class="ledger-rule" d="${grid}"/>${labels}${areas}<g filter="url(#pen-marker-lite)">${ink}</g><path fill="${input.ink ?? "var(--pen-clay)"}" fill-opacity="0.8" d="${carried}"/>${nowLine}${resetMark}${hits}</svg>`,
    said,
  };
}

export interface WeeksInput {
  width: number;
  height: number;
  /** The reset weeks shown, oldest first, the current one last, each
   *  person's amount in it; and of the seven-day window, how much the
   *  account used and each person's part, where the reader sees them. */
  columns: { start: number; end: number; values: Map<number, number>; reached: number | null; parts: Map<number, number> }[];
  people: Person[];
  amounts: Amounts;
  words: UsageWords;
  seed: string;
  /** A weekly allowance to rule across, in the amounts' unit, and how it
   *  is written. */
  cap: { value: number; said: string } | null;
  /** The one person the bars are narrowed to. */
  only: { index: number; ink: string } | null;
}

/** A bar a reset week, stacked by person in each one's ink, over it how
 *  much of its seven-day window the account used, and the part of the one
 *  the bars are narrowed to; a weekly allowance as a dashed rule across. */
export function weeks(input: WeeksInput): string {
  const { width, height, columns, people, amounts, words, cap, only } = input;
  const top = 26;
  const bottom = 22;
  const left = 46;
  const right = 6;
  const w = width - left - right;
  const h = height - top - bottom;
  const most = Math.max(1e-9, cap?.value ?? 0, ...columns.map((column) => total(column.values)));
  const scale = nice(most);
  const y = (value: number) => top + h - (value / scale) * h;
  const random = seeded(`daily:${input.seed}`);
  let grid = "";
  let labels = "";
  for (const share of [0, 0.5, 1]) {
    grid += ruled([left, y(scale * share)], [left + w, y(scale * share)], share === 0 ? 1 : 0.6, random);
    labels += `<text class="ledger-axis" x="${left - 6}" y="${f(y(scale * share) + 4)}" text-anchor="end">${share === 0 ? "0" : escape(amounts.axis(scale * share))}</text>`;
  }
  const n = columns.length;
  const slot = w / Math.max(1, n);
  const thick = Math.max(3, Math.min(36, slot * 0.62));
  // As many dates as fit under the bars, counted back from this week.
  const every = Math.max(1, Math.ceil(44 / slot));
  let ink = "";
  let hits = "";
  if (cap && cap.value > 0) {
    grid += dashed([left, y(cap.value)], [left + w, y(cap.value)], 1.1, random, 7, 5);
    labels += `<text class="ledger-axis strong" x="${f(left + w)}" y="${f(y(cap.value) - 5)}" text-anchor="end">${escape(words.cap(cap.said))}</text>`;
  }
  columns.forEach((column, index) => {
    const cx = left + slot * index + slot / 2;
    let below = 0;
    for (const person of people) {
      const value = column.values.get(person.index) ?? 0;
      if (value <= 0) continue;
      const y0 = y(below);
      const y1 = y(below + value);
      below += value;
      ink += `<path fill="${person.ink}" d="${swatch(cx - thick / 2, y1, thick, Math.max(1.5, y0 - y1), random, 2)}"/>`;
    }
    const at = new Date(column.start);
    const current = index === n - 1;
    const label = current ? words.thisWeekShort : (n - 1 - index) % every === 0 ? `${at.getMonth() + 1}/${at.getDate()}` : "";
    if (label) labels += `<text class="ledger-axis${current ? " strong" : ""}" x="${f(cx)}" y="${height - 6}" text-anchor="middle">${escape(label)}</text>`;
    const sum = total(column.values);
    const pct = (share: number) => Math.round(share * 100);
    // Narrowed to one person, a week they did not use says nothing.
    if (column.reached !== null && slot >= 30 && (!only || sum > 0)) {
      const mine = only ? `<tspan fill="${only.ink}">${pct(column.parts.get(only.index) ?? 0)}%</tspan><tspan> / </tspan>` : "";
      labels += `<text class="ledger-axis ledger-reached" x="${f(cx)}" y="${f(y(sum) - 6)}" text-anchor="middle">${mine}<tspan>${pct(column.reached)}%</tspan></text>`;
    }
    const reached =
      column.reached === null
        ? ""
        : `\n${words.weekReached(pct(column.reached))}${people.filter((person) => column.parts.has(person.index)).map((person) => `\n${words.partOf(person.name, pct(column.parts.get(person.index)!))}`).join("")}`;
    const said = `${words.weekSpan(at, new Date(column.end))}\n${sum ? amounts.say(sum) : words.unused}${split(column.values, people, amounts)}${reached}`;
    hits += `<rect class="ledger-hit" x="${f(cx - Math.min(slot, thick + 18) / 2)}" y="${top}" width="${f(Math.min(slot, thick + 18))}" height="${h + 4}" rx="4"${hint(said)}/>`;
  });
  return `<svg class="ledger-chart" viewBox="0 0 ${f(width)} ${height}" width="${f(width)}" height="${height}">${hits}<path class="ledger-rule" d="${grid}"/><g class="ledger-ink" filter="url(#pen-marker-lite)">${ink}</g>${labels}</svg>`;
}

/** The people of a stacked drawing, named once in their inks, ending at
 *  `right` on the line `y`. */
function peopleKeys(people: Person[], right: number, y: number): string {
  let x = right;
  let keys = "";
  for (const person of [...people].reverse()) {
    const width = person.name.length * 7.5 + 4;
    x -= width;
    keys += `<text class="ledger-axis" x="${f(x + 13)}" y="${f(y + 4)}">${escape(person.name)}</text><rect x="${f(x)}" y="${f(y - 4)}" width="9" height="9" rx="2.5" fill="${person.ink}"/>`;
    x -= 16;
  }
  return keys;
}

/** A round top for an axis: 1, 2, 5 or 10 times a power of ten. */
function nice(value: number): number {
  const power = 10 ** Math.floor(Math.log10(value));
  const step = [1, 2, 5, 10].find((one) => one * power >= value) ?? 10;
  return step * power;
}

export interface PunchInput {
  width: number;
  height: number;
  /** Seven rows of 24, Monday first. */
  grid: number[][];
  ink: string;
  amounts: Amounts;
  words: UsageWords;
  seed: string;
}

/** The hours of the week as dots, each as large as the use in it. */
export function punch(input: PunchInput): string {
  const { width, height, grid, ink, amounts, words } = input;
  const left = 34;
  const top = 6;
  const bottom = 22;
  const w = width - left - 6;
  const h = height - top - bottom;
  const cw = w / 24;
  const ch = h / 7;
  const largest = Math.min(cw, ch) * 0.46;
  const most = Math.max(1e-9, ...grid.flat());
  const random = seeded(`punch:${input.seed}`);
  let base = "";
  let dots = "";
  let labels = "";
  let hits = "";
  for (let weekday = 0; weekday < 7; weekday += 1) {
    labels += `<text class="ledger-axis" x="${left - 8}" y="${f(top + ch * weekday + ch / 2 + 4)}" text-anchor="end">${escape(words.weekday[weekday]!)}</text>`;
    for (let hour = 0; hour < 24; hour += 1) {
      const cx = left + cw * hour + cw / 2;
      const cy = top + ch * weekday + ch / 2;
      const value = grid[weekday]![hour]!;
      base += dot([cx, cy], 1.1, random);
      if (value > most * 0.02) dots += dot([cx, cy], Math.max(2, Math.sqrt(value / most) * largest), random);
      hits += `<rect x="${f(cx - cw / 2)}" y="${f(cy - ch / 2)}" width="${f(cw)}" height="${f(ch)}" fill="transparent"${hint(words.punchAt(words.weekday[weekday]!, hour, value ? amounts.say(value) : null))}/>`;
    }
  }
  const marks = width < 520 ? [0, 6, 12, 18] : [0, 3, 6, 9, 12, 15, 18, 21];
  for (const hour of marks) labels += `<text class="ledger-axis" x="${f(left + cw * hour + cw / 2)}" y="${height - 6}" text-anchor="middle">${hour}:00</text>`;
  return `<svg class="ledger-chart" viewBox="0 0 ${f(width)} ${height}" width="${f(width)}" height="${height}"><path class="ledger-trace" d="${base}"/><g filter="url(#pen-marker-lite)"><path fill="${ink}" fill-opacity="0.9" d="${dots}"/></g>${labels}${hits}</svg>`;
}

/** One bar of parts, each a felt stroke in its ink as long as its share,
 *  laid end to end. */
export function parts(shares: { value: number; ink: string; said: string }[], seed: string, width = 480): string {
  const random = seeded(`parts:${seed}`);
  const sum = shares.reduce((all, one) => all + one.value, 0) || 1;
  const ink = strokes(shares.map((one) => ({ share: one.value / sum, fill: one.ink })), 2, width - 4, 11, 16, random);
  let x = 2;
  let hits = "";
  for (const one of shares) {
    const length = (one.value / sum) * (width - 4);
    hits += `<rect x="${f(x)}" y="0" width="${f(Math.max(0, length))}" height="22" fill="transparent"${hint(one.said)}/>`;
    x += length;
  }
  return marked(
    `<svg class="ledger-parts" viewBox="0 0 ${width} 22" preserveAspectRatio="none"><g filter="url(#pen-felt)">${ink}</g>${hits}</svg>`,
    { kind: "parts", shares, seed },
    width,
  );
}

/** A dashed rule, for an empty chart's baseline. */
export function emptyLine(width: number, seed: string): string {
  return `<svg class="ledger-bar" viewBox="0 0 ${width} 6" preserveAspectRatio="none" aria-hidden="true"><path class="ledger-trace" d="${dashed([2, 3], [width - 2, 3], 1, seeded(seed))}"/></svg>`;
}
