// Model collaboration's film (`world-gui.md`, 功能). You and an agent build
// a thing together; one part that has to be done thoroughly it hands to a
// GPT on its own machine; the camera goes there while the part is worked
// through; it comes back and drops into its place in the whole. Seventeen
// seconds, wide and tall.

import type { Point } from "../pen/shapes";
import { Reel, type Built, type Pose, type Shape } from "./reel";

export interface CollaborationWords {
  captions: readonly [string, string, string, string];
}

interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}

interface Layout {
  width: number;
  height: number;
  /** The conversation with the agent. */
  talk: Rect;
  /** The whole being built: four parts, the last the one handed over. */
  parts: Point[];
  part: { w: number; h: number };
  /** The GPT's machine: its screen. */
  machine: Rect;
  /** Where the handed part waits beside it. */
  away: Point;
  /** The camera, close on the machine. */
  close: Pose;
  label: number;
}

const WIDE: Layout = {
  width: 1600,
  height: 900,
  talk: { x: 80, y: 150, w: 420, h: 600 },
  parts: [
    [600, 200],
    [920, 200],
    [600, 480],
    [920, 480],
  ],
  part: { w: 250, h: 130 },
  machine: { x: 1290, y: 320, w: 230, h: 150 },
  away: [1405, 205],
  close: { t: 0, x: -605, y: 145, sx: 1.7, sy: 1.7 },
  label: 26,
};

const TALL: Layout = {
  width: 900,
  height: 1200,
  talk: { x: 60, y: 40, w: 780, h: 300 },
  parts: [
    [140, 410],
    [490, 410],
    [140, 640],
    [490, 640],
  ],
  part: { w: 270, h: 140 },
  machine: { x: 400, y: 930, w: 240, h: 150 },
  away: [235, 1000],
  close: { t: 0, x: 70, y: -400, sx: 1.45, sy: 1.45 },
  label: 32,
};

export function collaboration(words: CollaborationWords, shape: Shape): Built {
  const L = shape === "wide" ? WIDE : TALL;
  const reel = new Reel(`collaboration-${shape}`, L.width, L.height);
  const [first, second, third, fourth] = words.captions;
  reel.say(200, first);
  reel.say(4600, second);
  reel.say(8700, third);
  reel.say(12900, fourth);

  const { talk: t, part, machine: m } = L;
  const mc: Point = [m.x + m.w / 2, m.y + m.h / 2];
  const focus: Point = shape === "wide" ? [1400, 320] : [380, 1000];
  // The camera goes close to the GPT's machine and comes back.
  reel.group(focus, [{ t: 0 }, { t: 8300 }, { ...L.close, t: 9200 }, { ...L.close, t: 12300 }, { t: 13200, x: 0, y: 0, sx: 1, sy: 1 }], () => {
    // You and the agent, talking.
    reel.wash(t.x, t.y, t.w, t.h, "page", 200, 420, 24);
    reel.wash(t.x + t.w - 280, t.y + 32, 250, 58, "bubble", 500, 280, 20);
    reel.scribble([t.x + t.w - 256, t.y + 62], 190, "ink", 700, 420);
    reel.mark("claude", [t.x + 26, t.y + 120], 44, "clay", 1100);
    reel.scribble([t.x + 86, t.y + 144], t.w - 130, "ink", 1250, 600);
    reel.scribble([t.x + 86, t.y + 184], t.w - 200, "ink", 1700, 500);
    reel.wash(t.x + t.w - 230, t.y + (shape === "wide" ? 250 : 222), 200, 52, "bubble", 2600, 260, 18);
    reel.scribble([t.x + t.w - 206, t.y + (shape === "wide" ? 278 : 250)], 140, "ink", 2760, 360);

    // The whole, growing as they talk: parts and what joins them.
    const centreOf = (index: number): Point => [L.parts[index]![0] + part.w / 2, L.parts[index]![1] + part.h / 2];
    const joins: [number, number][] = [
      [0, 1],
      [0, 2],
      [1, 3],
      [2, 3],
    ];
    L.parts.slice(0, 3).forEach(([x, y], index) => {
      reel.wash(x, y, part.w, part.h, "raised", 900 + index * 650, 360, 16);
      reel.scribble([x + 24, y + 42], part.w * 0.55, "ink", 1050 + index * 650, 360, 3);
      reel.scribble([x + 24, y + 82], part.w * 0.7, "graphite", 1200 + index * 650, 400, 2.6);
    });
    joins.forEach(([a, b], index) => {
      const [ax, ay] = centreOf(a);
      const [bx, by] = centreOf(b);
      const across = ay === by;
      const from: Point = across ? [ax + part.w / 2 + 6, ay] : [ax, ay + part.h / 2 + 6];
      const to: Point = across ? [bx - part.w / 2 - 6, by] : [bx, by - part.h / 2 - 6];
      reel.line(from, to, 3.4, "graphite", 2300 + index * 280, 300);
      // Once the part is back, the joins are inked over.
      reel.line(from, to, 4.2, "ink", 14400 + index * 220, 260);
    });

    // The part that has to be done thoroughly: ringed, then handed over,
    // a dashed place left where it was.
    const [px, py] = L.parts[3]!;
    const slot: Point = centreOf(3);
    const hole: Point[] = [
      [px - 10, py - 10],
      [px + part.w + 10, py - 10],
      [px + part.w + 10, py + part.h + 10],
      [px - 10, py + part.h + 10],
      [px - 10, py - 10],
    ];
    reel.dashes(hole, 22, 3.4, "clay", 4700, 700);
    const dx = L.away[0] - slot[0];
    const dy = L.away[1] - slot[1];
    const lift = shape === "wide" ? -120 : 0;
    const arc = curve(slot, [slot[0] + dx / 2 + (shape === "wide" ? 0 : -160), slot[1] + dy / 2 + lift], L.away);
    reel.dashes(arc, 26, 3.4, "graphite", 5700, 800);
    const there: Pose = { t: 7000, x: dx, y: dy, sx: 0.8, sy: 0.8 };
    reel.group(slot, [{ t: 0 }, { t: 5600 }, { t: 5900, y: -12, sx: 1.05, sy: 1.05 }, { t: 6450, x: dx / 2, y: dy / 2 + lift, sx: 0.9, sy: 0.9 }, there, { ...there, t: 13000 }, { t: 13700, x: dx / 2, y: dy / 2 + lift, sx: 0.9, sy: 0.9 }, { t: 14200, x: 0, y: -12, sx: 1.05, sy: 1.05 }, { t: 14450, x: 0, y: 0, sx: 1, sy: 1 }], () => {
      // A part like the others, until it is ringed: then it takes the clay.
      reel.wash(px, py, part.w, part.h, "raised", 2850, 360, 16);
      reel.group([px, py], [{ t: 4700, o: 0 }, { t: 5200, o: 1 }], () => reel.wash(px, py, part.w, part.h, "clay", 4700, 400, 16));
      reel.scribble([px + 24, py + 42], part.w * 0.5, "ink", 3000, 360, 3);
      reel.scribble([px + 24, py + 82], part.w * 0.62, "graphite", 3150, 380, 2.6);
      // Worked through, stroke after stroke, on the GPT's machine.
      for (let index = 0; index < 22; index += 1) {
        const x = px + 18 + ((index * 37) % (part.w - 60));
        const y = py + 66 + ((index * 23) % (part.h - 80));
        reel.line([x, y + 14], [x + 26, y], 2.2, index % 4 === 0 ? "clay" : "ink", 9400 + index * 105, 120);
      }
      for (let index = 0; index < 3; index += 1) reel.check([px + part.w + 22, py + 30 + index * 38], 22, "clay", 11800 + index * 220);
    });

    // The GPT's machine.
    reel.wash(m.x, m.y, m.w, m.h, "raised", 5200, 360, 14);
    reel.wash(m.x - 34, m.y + m.h - 2, m.w + 68, 22, "panel", 5400, 260, 11);
    reel.mark("codex", [mc[0] - 36, mc[1] - 40], 72, "graphite", 5600);
    reel.text([mc[0], m.y + m.h + 66], "laptop", L.label, "graphite", 5800, { anchor: "middle" });

    // Back in the conversation: a word from the agent, and a tick.
    reel.scribble([t.x + 86, t.y + (shape === "wide" ? 360 : 296)], t.w - 170, "ink", 15000, 600);
    reel.check([t.x + 34, t.y + (shape === "wide" ? 362 : 298)], 28, "clay", 15700);
  });

  return reel.build(17000);
}

function curve(from: Point, bend: Point, to: Point): Point[] {
  const points: Point[] = [];
  for (let index = 0; index <= 40; index += 1) {
    const t = index / 40;
    const v = 1 - t;
    points.push([v * v * from[0] + 2 * v * t * bend[0] + t * t * to[0], v * v * from[1] + 2 * v * t * bend[1] + t * t * to[1]]);
  }
  return points;
}
