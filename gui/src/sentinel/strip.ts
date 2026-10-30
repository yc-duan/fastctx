// A watcher's time, drawn (`world-gui.md`, 观测), as an instrument traces
// it: one line across, from its first look to now and on to when it next
// looks. Asleep is the quiet line; its reflex's rhythm is ink dots along
// it, the closer the more often it looks; each wake is a blot, an open ring
// while it is still awake, and where a reflex saw something move, the line
// trembles just before it; now is an upright stroke, what is to come a
// dashed line of its own short length to a small ring, the past drawn to
// scale before it; what it found is ringed in clay, the one stroke of
// colour. The wake being read below is marked by a short stroke
// under it. No words on it: times and rhythms are said where the pointer
// rests, and a wake pressed is read.

import { blot, circle, dashed, dot, outline, ruled, seeded, type Point } from "../pen/shapes";
import type { SessionEvent, WokeWhy } from "../world/events";
import { escape } from "../conversation/prose";

/** One wake as the strip draws it. */
export interface Wake {
  turn: string;
  at: number;
  why: WokeWhy | null;
  /** Its turn had ended when read. */
  ended: boolean;
  /** How often the reflex it set looks, where it set one. */
  every: number | null;
}

/** The wakes in a record's events, in order. */
export function wakesOf(events: SessionEvent[]): Wake[] {
  const wakes: Wake[] = [];
  const byTurn = new Map<string, Wake>();
  for (const event of events) {
    if (event.event === "turn_started") {
      const wake: Wake = { turn: event.turn, at: Date.parse(event.at), why: null, ended: false, every: null };
      wakes.push(wake);
      byTurn.set(event.turn, wake);
    } else if (event.event === "turn_ended") {
      const wake = byTurn.get(event.turn);
      if (wake) wake.ended = true;
    } else if (event.event === "heard" && event.turn) {
      const wake = byTurn.get(event.turn);
      const notice = event.notice;
      if (wake && notice?.kind === "world" && notice.fact.fact === "woke") wake.why = notice.fact.why;
    } else if (event.event === "finished") {
      const wake = byTurn.get(event.turn);
      const item = event.item;
      if (wake && item.kind === "tool" && item.name === "install_reflex" && item.input && typeof item.input === "object" && !Array.isArray(item.input)) {
        const every = (item.input as Record<string, unknown>).every_ms;
        if (typeof every === "number" && every > 0) wake.every = every;
      }
    }
  }
  return wakes;
}

export interface StripDraw {
  wakes: Wake[];
  now: number;
  /** When it next looks or is woken, while it still watches. */
  next: number | null;
  /** It found what it watched for: its last wake is ringed. */
  found: boolean;
  /** It ended, found or not: nothing is drawn ahead of its last wake. */
  over: boolean;
  width: number;
  height: number;
  seed: string;
  /** What the pointer resting on a mark is told. */
  said: { at(time: number): string; next(time: number): string; every(ms: number): string; woke(why: WokeWhy): string };
}

const PAD = 20;
const LEAST_SPAN = 30 * 60_000;
/** How far before its wake a tremor begins. */
const TREMOR = 16;
/** How long the line to what is to come is drawn, whenever that is. */
const AHEAD = 40;
const f = (n: number) => n.toFixed(1);

/** The strip as SVG content for a viewBox of `width` by `height`. */
export function stripMarkup(draw: StripDraw): string {
  const { wakes, now, width, height, said } = draw;
  // Each mark keeps its own hand, so a mark drawn again is drawn the same.
  const pen = (key: string) => seeded(`${draw.seed}:${key}`);
  const y = Math.round(height / 2);
  const last = wakes[wakes.length - 1];
  const first = wakes[0]?.at ?? now;
  const reach = draw.over ? (last?.at ?? now) : now;
  const ahead = !draw.over && draw.next !== null && draw.next > now ? draw.next : null;
  // The past to scale up to `right`; what is to come after it, at a length
  // of its own, so marks close to now never crowd it.
  const right = width - PAD - (ahead !== null ? AHEAD : 0);
  const span = Math.max(LEAST_SPAN, reach - first);
  const end = first + span * 1.04;
  const scale = (right - PAD) / (end - first);
  const x = (time: number) => PAD + (Math.min(time, end) - first) * scale;
  const parts: string[] = [];
  const hits: string[] = [];
  const shape = (d: string, kind: string, extra = "") => parts.push(`<path class="${kind}"${extra} d="${d}"/>`);

  // Asleep: one line from its first look to where it stops or now, broken
  // where it trembles before a wake.
  const trembles = wakes.filter((wake) => wake.why?.kind === "reflex").map((wake) => x(wake.at));
  let from = x(first);
  for (const [index, to] of [...trembles.map((at) => at - TREMOR), x(reach)].entries()) {
    if (to - from > 2) shape(ruled([from, y], [to, y], 1.1, pen(`line:${index}`)), "sn-sleep");
    from = Math.max(from, (trembles[index] ?? to) - 2.5);
  }

  // Each reflex's rhythm, from the wake that set it to the next wake.
  wakes.forEach((wake, index) => {
    if (!wake.every) return;
    const to = wakes[index + 1]?.at ?? reach;
    const step = wake.every * scale;
    const x0 = x(wake.at) + 7;
    const x1 = x(to) - (wakes[index + 1]?.why?.kind === "reflex" ? TREMOR + 3 : 7);
    if (x1 <= x0) return;
    const hand = pen(`beat:${wake.turn}`);
    if (step < 5) {
      // Too often to tell apart: a firmer line says so.
      shape(ruled([x0, y], [x1, y], 2.2, hand), "sn-beat");
    } else {
      for (let at = x0 + step; at < x1; at += step) shape(dot([at, y], 1.15, hand), "sn-beat");
    }
    hits.push(`<rect class="sn-hit-span" x="${f(x0)}" y="${y - 9}" width="${f(x1 - x0)}" height="18"><title>${escape(said.every(wake.every))}</title></rect>`);
  });

  // What is to come: a dashed line to when it next looks.
  if (ahead !== null) {
    const xNext = width - PAD;
    if (xNext - x(now) > 10) shape(dashed([x(now) + 5, y], [xNext - 4, y], 1, pen("ahead"), 4, 4), "sn-ahead");
    shape(circle([xNext, y], 3.1, 1.1, pen("next")), "sn-next");
    hits.push(`<circle class="sn-hit sn-hit-next" cx="${f(xNext)}" cy="${y}" r="12"><title>${escape(said.next(ahead))}</title></circle>`);
  }

  // Now: an upright stroke, where it still watches; while it is awake, its
  // open ring is now.
  const awakeNow = last !== undefined && !last.ended && x(now) - x(last.at) < 8;
  if (!draw.over && !awakeNow) shape(ruled([x(now), y - 11], [x(now), y + 11], 1.3, pen("now")), "sn-now");

  // The wakes, over the line; under each, the stroke that marks it read.
  wakes.forEach((wake, index) => {
    const at: [number, number] = [x(wake.at), y];
    const hand = pen(`wake:${wake.turn}`);
    const broken = wake.why?.kind === "broken";
    if (wake.why?.kind === "reflex") shape(tremor(at, hand), "sn-tremor");
    if (!wake.ended) shape(circle(at, 4.2, 1.4, hand), "sn-wake-open");
    else shape(blot(at, index === 0 ? 4.4 : 3.7, hand), broken ? "sn-wake-wrong" : "sn-wake");
    if (draw.found && index === wakes.length - 1) shape(circle(at, 9.5, 1.5, hand), "sn-found");
    shape(ruled([at[0] - 5, y + 14], [at[0] + 5, y + 14], 1.4, hand), "sn-here", ` data-here="${escape(wake.turn)}"`);
    const told = `${said.at(wake.at)}${wake.why ? `\n${said.woke(wake.why)}` : ""}`;
    hits.push(
      `<circle class="sn-hit" cx="${f(at[0])}" cy="${y}" r="12" data-turn="${escape(wake.turn)}" tabindex="0" role="button" aria-label="${escape(told)}"><title>${escape(told)}</title></circle>`,
    );
  });

  return `${parts.join("")}${hits.join("")}`;
}

/** The line trembling just before a wake, as a needle does when what it
 *  rests on moves: a few quick swings, the largest in the middle. */
function tremor(at: Point, random: () => number): string {
  const [x, y] = at;
  const swing = (share: number) => share * (10 + random() * 3);
  const points: Point[] = [
    [x - TREMOR, y],
    [x - 13.5, y - swing(0.3)],
    [x - 11, y + swing(0.45)],
    [x - 8.5, y - swing(1)],
    [x - 6, y + swing(0.6)],
    [x - 4, y - swing(0.2)],
    [x - 2.5, y],
  ];
  return outline(points, 1.2, random, { pressure: 0.2, taper: 0.15 });
}
