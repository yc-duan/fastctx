// The machines page's alerts (`world-gui.md`, 机器页): what the hub told of
// within the week it keeps them, a row for each machine with troubles and
// the network's own first, each trouble a short stroke along a ruled week,
// a fault's in the colour of a fault and the rest in a warning's. What each
// was is behind a pointer's rest, or a long press on a phone; a row says
// all of its own the same way and opens its machine's page.

import { glyph, type GlyphName } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import { outline, ruled, seeded } from "../pen/shapes";
import type { Trouble } from "../world/accounts";
import type { WorldNow } from "../world/bridge";
import { knot } from "./machine-list";
import { link } from "./places";
import { machinesOf } from "./reach";
import { troubleTone, type TroubleWords } from "./trouble-words";

export const DAY_MS = 86_400_000;
/** The week the hub keeps troubles for. */
const WEEK_MS = 7 * DAY_MS;
/** The most troubles a row's summary names. */
const SUMMARY = 6;

export interface TroublesNow {
  world: WorldNow;
  now: Date;
  /** When something was, as the list says it. */
  when(at: Date, now: Date): string;
}

/** How many troubles of the network shown fall within `span` of now, but
 *  those over. */
export function troublesIn(world: WorldNow, now: Date, span = WEEK_MS): number {
  const from = now.getTime() - span;
  return world.accounts.hubs.reduce((sum, hub) => sum + (hub.troubles ?? []).filter((trouble) => !trouble.resolved && Date.parse(trouble.at) >= from).length, 0);
}

/** Whether a hub of the network shown tells troubles at all. */
export function tellsTroubles(world: WorldNow): boolean {
  return world.accounts.hubs.some((hub) => hub.health && hub.logins.length);
}

interface Row {
  key: string;
  label: string;
  icon: string;
  href: string | null;
  troubles: Trouble[];
}

/** The days drawn: the week back from today, whole, and today, so that
 *  each day is a column of its own and today the last. */
function daysOf(now: Date): { start: number; end: number; today: number } {
  const today = new Date(now);
  today.setHours(0, 0, 0, 0);
  const start = new Date(today);
  start.setDate(start.getDate() - 7);
  const end = new Date(today);
  end.setDate(end.getDate() + 1);
  return { start: start.getTime(), end: end.getTime(), today: today.getTime() };
}

export function troubleView(now: TroublesNow, words: TroubleWords): string {
  const { start, end, today } = daysOf(now.now);
  const machines = new Map(machinesOf(now.world).map((machine) => [machine.name, machine]));
  const rows = new Map<string, Row>();
  for (const hub of now.world.accounts.hubs) {
    for (const trouble of hub.troubles ?? []) {
      const at = Date.parse(trouble.at);
      if (!(at >= start && at < end)) continue;
      const name = trouble.machine ?? null;
      const key = name ? `m:${name}` : `hub:${hub.hub}`;
      let row = rows.get(key);
      if (!row) {
        const machine = name ? machines.get(name) : undefined;
        const facts = machine && "Ok" in machine.facts ? machine.facts.Ok : null;
        const form: GlyphName = facts?.form === "laptop" ? "laptop" : facts?.form === "server" ? "server" : "desktop";
        row = { key, label: name ?? hub.name, icon: name ? glyph(form, 15) : knot(), href: name ? link({ kind: "machine", name }) : null, troubles: [] };
        rows.set(key, row);
      }
      row.troubles.push(trouble);
    }
  }
  if (!rows.size) return `<p class="tv-none">${escape(words.none)}</p>`;
  const latest = (row: Row) => Math.max(...row.troubles.map((trouble) => Date.parse(trouble.at)));
  // The network's own first, then whichever had trouble last.
  const ordered = [...rows.values()].sort((a, b) => Number(!b.href) - Number(!a.href) || latest(b) - latest(a));
  const x = (at: number) => `${(((Math.min(Math.max(at, start), end) - start) / (end - start)) * 100).toFixed(2)}%`;
  const said = (trouble: Trouble) => `${now.when(new Date(trouble.at), now.now)}　${words.what(trouble)}${trouble.resolved ? `，${words.over}` : ""}`;
  const body = ordered
    .map((row) => {
      const newest = [...row.troubles].sort((a, b) => Date.parse(b.at) - Date.parse(a.at));
      const summary = [row.label, ...newest.slice(0, SUMMARY).map(said), newest.length > SUMMARY ? "⋯" : ""].filter(Boolean).join("\n");
      const marks = newest
        .map((trouble) => {
          const detail = words.detail(trouble);
          const random = seeded(`trouble:${trouble.id}:${trouble.at}`);
          const stroke = outline([[4, 1.5], [4, 14.5]], 2.6, random, { pressure: 0.2, taper: 0.15 });
          return `<span class="tv-mark ${troubleTone(trouble)}" style="left:${x(Date.parse(trouble.at))}" title="${escape([said(trouble), detail].filter(Boolean).join("\n"))}"><svg viewBox="0 0 8 16" width="8" height="16" aria-hidden="true"><path d="${stroke}"/></svg></span>`;
        })
        .join("");
      const inner = `<span class="tv-who">${row.icon}<span class="tv-name">${escape(row.label)}</span></span><span class="tv-strip">${rule(row.key)}${marks}</span>`;
      const label = escape(summary.replace(/\n/g, "；"));
      return row.href
        ? `<li><a class="tv-row" href="${row.href}" title="${escape(summary)}" aria-label="${label}">${inner}</a></li>`
        : `<li><div class="tv-row" title="${escape(summary)}" aria-label="${label}">${inner}</div></li>`;
    })
    .join("");
  return `<div class="tv-head" aria-hidden="true"><span class="tv-who"></span><span class="tv-days">${ruler(start, end, today, words)}</span></div><ol class="tv-rows scroller">${body}</ol>`;
}

/** A row's ruled line, drawn once at a width it is stretched from. */
function rule(seed: string): string {
  const random = seeded(`trouble-rule:${seed}`);
  return `<svg class="tv-rule" viewBox="0 0 360 16" preserveAspectRatio="none" aria-hidden="true"><path d="${ruled([2, 8], [358, 8], 0.6, random)}"/></svg>`;
}

/** The days, each named under its middle, in full where there is room and
 *  by its first letter where not, a short stroke at each midnight. */
function ruler(start: number, end: number, today: number, words: TroubleWords): string {
  const midnights: number[] = [];
  const cursor = new Date(today);
  while (cursor.getTime() > start) {
    midnights.unshift(cursor.getTime());
    cursor.setDate(cursor.getDate() - 1);
  }
  const bounds = [start, ...midnights, end];
  const at = (time: number) => `${(((time - start) / (end - start)) * 100).toFixed(2)}%`;
  const names = bounds
    .slice(0, -1)
    .map((from, index) => {
      const to = bounds[index + 1]!;
      const day = new Date(from).getDay();
      const [whole, short] = from >= today ? [words.today, words.today] : [words.weekday(day), words.weekdayShort(day)];
      return `<span class="tv-day" style="left:${at((from + to) / 2)}"><span class="tv-whole">${escape(whole)}</span><span class="tv-short">${escape(short)}</span></span>`;
    })
    .join("");
  const ticks = midnights
    .map((time) => {
      const stroke = outline([[2, 1], [2, 7]], 1, seeded(`trouble-tick:${time}`), { pressure: 0.15, taper: 0.3 });
      return `<span class="tv-tick" style="left:${at(time)}"><svg viewBox="0 0 4 8" width="4" height="8" aria-hidden="true"><path d="${stroke}"/></svg></span>`;
    })
    .join("");
  return names + ticks;
}
