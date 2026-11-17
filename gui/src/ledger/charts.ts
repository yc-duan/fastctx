// The ledger's drawings of what was used: a bar a day or a week, stacked by
// whatever it is split by; a year as a calendar; the hours of a week as
// dots. Each is an SVG drawn at the width it is shown at, in the pen's inks
// and by its hand (`pen/shapes.ts`). What a mark says is kept on it
// (`data-said`) and written over the chart while the pointer is on the mark
// (`readout`): above the marks, never on them.

import { escape } from "../conversation/prose";
import { reducedMotion } from "../motion";
import { dot, ruled, seeded, swatch } from "../pen/shapes";

/** The inks of what a drawing is split into, in a fixed order: the same
 *  name keeps its ink whatever else is shown. What is not split is drawn in
 *  one ink, the pencil's, and only its newest bar in clay. */
export const INKS = ["var(--pen-clay)", "var(--pen-sky)", "var(--pen-sage)", "var(--pen-rose)", "var(--pen-graphite)"];

const f = (value: number) => (Math.round(value * 10) / 10).toString();

/** A chart's marks come onto the page: its bars stand up from the line they
 *  stand on, one after another from the left; its dots open out; a year's
 *  days come in a week at a time. Each frame is set by hand: an animation
 *  left to the style engine did not always start on a chart drawn again in
 *  place. Nothing moves where motion is off. */
export function rise(holder: HTMLElement): void {
  if (reducedMotion() || document.documentElement.hasAttribute("data-reduced")) return;
  const order = (mark: SVGElement) => Number(mark.style.getPropertyValue("--i")) || 0;
  const marks = [
    ...Array.from(holder.querySelectorAll<SVGElement>(".lgc-col"), (mark) => ({ mark, wait: order(mark) * 14, set: (grown: number) => (mark.style.scale = `1 ${grown}`) })),
    ...Array.from(holder.querySelectorAll<SVGElement>(".lgc-dot"), (mark) => ({ mark, wait: order(mark) * 16, set: (grown: number) => (mark.style.scale = `${grown}`) })),
    ...Array.from(holder.querySelectorAll<SVGElement>(".lgc-day"), (mark) => ({ mark, wait: order(mark) * 9, set: (grown: number) => (mark.style.opacity = `${grown}`) })),
  ];
  if (!marks.length) return;
  const LASTS = 520;
  for (const one of marks) one.set(0);
  const began = performance.now();
  let done = false;
  const stand = () => {
    done = true;
    for (const one of marks) {
      one.mark.style.scale = "";
      one.mark.style.opacity = "";
    }
  };
  const frame = (now: number) => {
    if (done) return;
    let moving = false;
    for (const one of marks) {
      const through = Math.min(1, Math.max(0, (now - began - one.wait) / LASTS));
      one.set(1 - (1 - through) ** 4);
      if (through < 1) moving = true;
    }
    if (moving && holder.isConnected) requestAnimationFrame(frame);
    else stand();
  };
  requestAnimationFrame(frame);
  // Whatever becomes of the frames, the marks end standing.
  window.setTimeout(stand, LASTS + Math.max(...marks.map((one) => one.wait)) + 300);
}

/** What the mark under the pointer says, written in the room over the
 *  chart's marks and never on them, and going along with the pointer from
 *  mark to mark. A chart split into parts says each part's figure in its
 *  legend meanwhile, in place of the legend's own. */
export function readout(holder: HTMLElement, legend: HTMLElement | null): void {
  const read = document.createElement("p");
  read.className = "lg-read";
  holder.append(read);
  const figures = legend ? Array.from(legend.querySelectorAll("b")) : [];
  const own = figures.map((figure) => figure.textContent ?? "");
  let on: Element | null = null;
  const leave = () => {
    if (!on) return;
    on = null;
    read.classList.remove("shown");
    legend?.classList.remove("reading");
    figures.forEach((figure, index) => (figure.textContent = own[index]!));
  };
  holder.addEventListener("pointermove", (event) => {
    const mark = (event.target as Element).closest("[data-said]");
    if (!mark) return leave();
    if (mark === on) return;
    on = mark;
    read.textContent = mark.getAttribute("data-said");
    const box = mark.getBoundingClientRect();
    const edge = holder.getBoundingClientRect();
    const half = read.offsetWidth / 2;
    read.style.setProperty("--slide", `${Math.round(Math.min(edge.width - half, Math.max(half, box.left + box.width / 2 - edge.left)))}px`);
    read.classList.add("shown");
    const parts = mark.getAttribute("data-parts")?.split("|");
    if (legend && parts) {
      legend.classList.add("reading");
      figures.forEach((figure, index) => (figure.textContent = parts[index] ?? ""));
    }
  });
  holder.addEventListener("pointerleave", leave);
}

/** A round figure at or above `value`, for the top of an axis. */
function roundUp(value: number): number {
  if (value <= 0) return 1;
  const power = 10 ** Math.floor(Math.log10(value));
  for (const step of [1, 1.5, 2, 3, 4, 5, 6, 8, 10]) if (step * power >= value) return step * power;
  return 10 * power;
}

export interface Column {
  /** What the column says under the pointer. */
  said: string;
  /** What each of its parts comes to, said in the legend meanwhile. */
  parts?: string[];
  values: number[];
  /** Written under it, where there is room for one. */
  tick?: string;
}

/** Bars standing on a ruled line, each stacked from its parts. */
export function bars(id: string, columns: Column[], width: number, height: number, axis: (value: number) => string): string {
  const random = seeded(`bars-${id}`);
  const left = 52;
  const top = 32;
  const bottom = 28;
  const plot = { w: width - left - 6, h: height - top - bottom };
  const most = roundUp(Math.max(...columns.map((column) => column.values.reduce((sum, value) => sum + value, 0)), 0.01));
  const slot = plot.w / columns.length;
  const wide = Math.min(slot * 0.66, 30);
  const y = (value: number) => top + plot.h - (value / most) * plot.h;
  let drawn = "";
  for (const part of [0, 0.5, 1]) {
    const at = y(most * part);
    drawn += `<path class="lgc-grid${part === 0 ? " lgc-floor" : ""}" d="${ruled([left - 4, at], [width - 2, at], part === 0 ? 1.1 : 0.8, random)}"/><text class="lgc-axis" x="${left - 12}" y="${f(at + 4)}" text-anchor="end">${escape(axis(most * part))}</text>`;
  }
  columns.forEach((column, index) => {
    const x = left + slot * index + (slot - wide) / 2;
    let from = 0;
    let inks = "";
    column.values.forEach((value, part) => {
      const tall = (value / most) * plot.h;
      // A part too thin to be a mark is left out; its figure is in the hint.
      const ink = column.values.length > 1 ? INKS[Math.min(part, INKS.length - 1)] : index === columns.length - 1 ? "var(--pen-clay)" : "var(--pen-graphite)";
      if (tall >= 1.6) inks += `<path style="fill:${ink}" d="${swatch(x, y(from + value) + (from ? 0 : 0), wide, Math.max(tall - (from ? 1.6 : 0), 1), random, 2)}"/>`;
      from += value;
    });
    const tick = column.tick ? `<text class="lgc-axis" x="${f(x + wide / 2)}" y="${height - 8}" text-anchor="middle">${escape(column.tick)}</text>` : "";
    drawn += `<g class="lgc-col" style="--i:${index}">${inks}</g>${tick}<rect class="lgc-hit" x="${f(left + slot * index)}" y="${top - 12}" width="${f(slot)}" height="${plot.h + 14}" rx="6" data-said="${escape(column.said)}"${column.parts ? ` data-parts="${escape(column.parts.join("|"))}"` : ""}/>`;
  });
  return `<svg class="lgc" viewBox="0 0 ${width} ${height}" width="${width}" height="${height}" role="img">${drawn}</svg>`;
}

/** A year of days, a week to a column, Monday at its top; a day is inked
 *  the deeper the more it used, by the quarter its amount falls in among
 *  the days that used anything. */
export function calendar(id: string, dates: Date[], values: number[], width: number, said: (index: number) => string): string {
  const left = 30;
  const top = 48;
  const first = (dates[0]!.getDay() + 6) % 7;
  const weeks = Math.ceil((first + dates.length) / 7);
  const pitch = Math.min(22, (width - left - 2) / weeks);
  const cell = pitch - 3.5;
  const used = values.filter((value) => value > 0).sort((a, b) => a - b);
  const quarter = (share: number) => used[Math.min(used.length - 1, Math.floor(used.length * share))] ?? 0;
  const steps = [quarter(0.25), quarter(0.5), quarter(0.75)];
  const random = seeded(`calendar-${id}`);
  let drawn = "";
  let month = -1;
  dates.forEach((date, index) => {
    const column = Math.floor((first + index) / 7);
    const row = (first + index) % 7;
    const x = left + column * pitch;
    if (row === 0 && date.getMonth() !== month && date.getDate() <= 7) {
      month = date.getMonth();
      drawn += `<text class="lgc-axis" x="${f(x)}" y="36">${month + 1} 月</text>`;
    }
    const value = values[index]!;
    const level = value <= 0 ? 0 : value <= steps[0]! ? 1 : value <= steps[1]! ? 2 : value <= steps[2]! ? 3 : 4;
    drawn += `<path class="lgc-day" style="--i:${column}" data-level="${level}" d="${swatch(x, top + row * pitch, cell, cell, random, 2.6)}" data-said="${escape(said(index))}"/>`;
  });
  ["一", "三", "五", "日"].forEach((name, index) => {
    drawn += `<text class="lgc-axis" x="0" y="${f(top + [0, 2, 4, 6][index]! * pitch + cell - 2)}">${name}</text>`;
  });
  const height = top + 7 * pitch;
  return `<svg class="lgc" viewBox="0 0 ${width} ${f(height)}" width="${width}" height="${f(height)}" role="img">${drawn}</svg>`;
}

/** Where a figure set by hand stands among what the weeks came to by the
 *  readings: a ruled line of dollars, a pencilled dot for each week, the
 *  newest the darkest, and the figure as a line across in clay, which can
 *  be dragged along. `x` is how a dollar figure is placed on it, for the
 *  one that moves the line. */
export function strip(id: string, weeks: { value: number; said: string }[], set: number, width: number): { svg: string; low: number; high: number; left: number; right: number } {
  const random = seeded(`strip-${id}`);
  const all = [...weeks.map((week) => week.value), set];
  const span = Math.max(Math.max(...all) - Math.min(...all), 600);
  const low = Math.max(0, Math.floor((Math.min(...all) - span * 0.35) / 500) * 500);
  const high = Math.ceil((Math.max(...all) + span * 0.35) / 500) * 500;
  const left = 10;
  const right = width - 10;
  const floor = 74;
  const x = (value: number) => left + ((value - low) / (high - low)) * (right - left);
  let drawn = `<path class="lgc-floor" d="${ruled([left - 6, floor], [right + 6, floor], 1.1, random)}"/>`;
  for (let tick = low; tick <= high; tick += 500) {
    const major = tick % 1000 === 0;
    drawn += `<path class="lgc-grid" d="${ruled([x(tick), floor - (major ? 5 : 3)], [x(tick), floor + (major ? 5 : 3)], 0.9, random)}"/>`;
    if (major) drawn += `<text class="lgc-axis" x="${f(x(tick))}" y="${floor + 22}" text-anchor="middle">$${tick.toLocaleString("en-US")}</text>`;
  }
  // Dots that would sit on one another are stacked.
  const placed: { x: number; level: number }[] = [];
  weeks.forEach((week, index) => {
    const at = x(week.value);
    let level = 0;
    while (placed.some((one) => one.level === level && Math.abs(one.x - at) < 11)) level += 1;
    placed.push({ x: at, level });
    drawn += `<path class="lgc-week${index === weeks.length - 1 ? " lgc-newest" : ""}" d="${dot([at, floor - 12 - level * 11], 4.4, random)}" title="${escape(week.said)}"/>`;
  });
  drawn += `<g class="lgc-set" transform="translate(${f(x(set))} 0)"><path d="${ruled([0, 20], [0, floor + 8], 2.2, random)}"/><text class="lgc-set-figure" x="0" y="13" text-anchor="middle">$${set.toLocaleString("en-US")}</text></g>`;
  return { svg: `<svg class="lgc lgc-strip" viewBox="0 0 ${width} 104" width="${width}" height="104" role="img">${drawn}</svg>`, low, high, left, right };
}

/** The hours of a week: a dot for each hour of each weekday, the larger the
 *  more was asked then. */
export function punch(id: string, hours: number[][], width: number, said: (weekday: number, hour: number) => string): string {
  const random = seeded(`punch-${id}`);
  const left = 44;
  const top = 34;
  const pitch = Math.min(40, (width - left - 8) / 24);
  const row = Math.min(pitch, 30);
  const most = Math.max(...hours.flat(), 1);
  let drawn = "";
  hours.forEach((day, weekday) => {
    const y = top + weekday * row + row / 2;
    drawn += `<text class="lgc-axis" x="0" y="${f(y + 4)}">${["周一", "周二", "周三", "周四", "周五", "周六", "周日"][weekday]}</text>`;
    day.forEach((value, hour) => {
      const x = left + hour * pitch + pitch / 2;
      const radius = Math.sqrt(value / most) * (row / 2 - 2);
      drawn += radius < 0.8 ? `<circle class="lgc-none" cx="${f(x)}" cy="${f(y)}" r="1"/>` : `<path class="lgc-dot" style="--i:${hour}" d="${dot([x, y], radius, random)}"/>`;
      drawn += `<rect class="lgc-hit" x="${f(x - pitch / 2)}" y="${f(y - row / 2)}" width="${f(pitch)}" height="${f(row)}" rx="6" data-said="${escape(said(weekday, hour))}"/>`;
    });
  });
  const foot = top + 7 * row + 18;
  for (const hour of [0, 6, 12, 18, 23]) drawn += `<text class="lgc-axis" x="${f(left + hour * pitch + pitch / 2)}" y="${f(foot)}" text-anchor="middle">${hour} 点</text>`;
  return `<svg class="lgc" viewBox="0 0 ${width} ${f(foot + 6)}" width="${width}" height="${f(foot + 6)}" role="img">${drawn}</svg>`;
}
