// Project memory's film (`world-gui.md`, 功能). A conversation writes down
// what mattered as a journal, sealed once written; days later another
// conversation opens and finds it at its first line; what was settled is a
// decision, and each change of it lays one more version over the last.
// Thirteen seconds, wide and tall.

import type { Point } from "../pen/shapes";
import { Reel, type Built, type Shape } from "./reel";

export interface MemoryFilmWords {
  captions: readonly [string, string, string];
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
  /** The conversation that writes the journal, and the one that reads it. */
  first: Rect;
  later: Rect;
  journal: Rect;
  spec: Rect;
}

const WIDE: Layout = {
  width: 1600,
  height: 900,
  first: { x: 80, y: 150, w: 400, h: 560 },
  later: { x: 1120, y: 150, w: 400, h: 560 },
  journal: { x: 640, y: 120, w: 320, h: 300 },
  spec: { x: 640, y: 500, w: 320, h: 250 },
};

const TALL: Layout = {
  width: 900,
  height: 1200,
  first: { x: 60, y: 40, w: 380, h: 360 },
  later: { x: 460, y: 40, w: 380, h: 360 },
  journal: { x: 290, y: 450, w: 320, h: 290 },
  spec: { x: 290, y: 820, w: 320, h: 250 },
};

export function memoryFilm(words: MemoryFilmWords, shape: Shape): Built {
  const L = shape === "wide" ? WIDE : TALL;
  const reel = new Reel(`memory-${shape}`, L.width, L.height);
  const [first, second, third] = words.captions;
  reel.say(200, first);
  reel.say(4400, second);
  reel.say(8600, third);

  // A conversation: its page, the agent's mark, a few lines of it.
  const talk = (r: Rect, mark: "claude" | "codex", at: number) => {
    reel.wash(r.x, r.y, r.w, r.h, "page", at, 420, 24);
    reel.mark(mark, [r.x + 24, r.y + 30], 40, mark === "claude" ? "clay" : "graphite", at + 300);
    reel.scribble([r.x + 80, r.y + 54], r.w - 130, "ink", at + 450, 520);
    reel.scribble([r.x + 80, r.y + 92], r.w - 180, "ink", at + 800, 460);
    reel.wash(r.x + r.w - 240, r.y + 140, 212, 50, "bubble", at + 1100, 260, 18);
    reel.scribble([r.x + r.w - 218, r.y + 166], 150, "ink", at + 1250, 360);
  };

  // A sheet: its name over a stroke, then its lines.
  const sheet = (r: Rect, at: number, lines: number) => {
    reel.wash(r.x, r.y, r.w, r.h, "raised", at, 380, 16);
    reel.scribble([r.x + 28, r.y + 44], r.w * 0.5, "ink", at + 250, 360, 3.6);
    reel.line([r.x + 28, r.y + 64], [r.x + r.w * 0.5 + 28, r.y + 63], 2, "graphite", at + 560, 220);
    for (let index = 0; index < lines; index += 1) reel.scribble([r.x + 28, r.y + 104 + index * 34], r.w * (index % 2 ? 0.62 : 0.74), "graphite", at + 700 + index * 190, 300, 2.6);
  };

  // What mattered in the first conversation, written down as a journal and
  // sealed once it is: a drop of clay at its foot.
  const f = L.first;
  const j = L.journal;
  talk(f, "claude", 200);
  reel.scribble([f.x + 80, f.y + 240], f.w - 150, "ink", 1700, 520);
  reel.dashes(curvePoints([f.x + f.w - 10, f.y + 250], [j.x - 40, j.y + 40], [j.x - 6, j.y + 120]), 22, 3.2, "graphite", 2000, 600);
  sheet(j, 2300, 5);
  reel.blot([j.x + j.w - 34, j.y + j.h - 32], 9, "clay", 3700);

  // Days later another conversation opens: the journal is at its first
  // line, the name a hand points it to.
  const l = L.later;
  talk(l, "codex", 4500);
  reel.wash(l.x + 70, l.y + 220, l.w - 100, 64, "panel", 5900, 300, 14);
  reel.mark("journal", [l.x + 86, l.y + 236], 32, "graphite", 6100);
  reel.scribble([l.x + 134, l.y + 254], l.w * 0.42, "ink", 6250, 380, 3);
  reel.dashes(curvePoints([j.x + j.w + 6, j.y + 120], [l.x - 40, j.y + 120], [l.x + 64, l.y + 252]), 22, 3.2, "clay", 6000, 700);
  reel.check([l.x + l.w - 52, l.y + 256], 24, "clay", 7400);

  // What was settled is a decision; each change lays a version over the
  // last, the newest ringed.
  const s = L.spec;
  for (let index = 0; index < 3; index += 1) {
    const at = 8700 + index * 1100;
    const shift = (2 - index) * 18;
    sheet({ x: s.x - shift, y: s.y - shift, w: s.w, h: s.h }, at, index === 2 ? 4 : 2);
  }
  reel.mark("spec", [s.x + s.w - 54, s.y + 20], 32, "graphite", 11200);
  reel.ring([s.x + s.w - 38, s.y + 36], 26, 2.6, "clay", 11500, 500);

  return reel.build(13000);
}

function curvePoints(from: Point, bend: Point, to: Point): Point[] {
  const points: Point[] = [];
  for (let index = 0; index <= 40; index += 1) {
    const t = index / 40;
    const v = 1 - t;
    points.push([v * v * from[0] + 2 * v * t * bend[0] + t * t * to[0], v * v * from[1] + 2 * v * t * bend[1] + t * t * to[1]]);
  }
  return points;
}
