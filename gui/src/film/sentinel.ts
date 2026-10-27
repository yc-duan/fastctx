// Sentinels' film (`world-senses.md`). A conversation that has to wait for
// something leaves a sentinel; it goes to the machine where the thing will
// show and watches its log while the conversation rests; when the line it
// waits for comes, it rings and the conversation wakes and goes on.
// Fourteen seconds, wide and tall.

import type { Point } from "../pen/shapes";
import { Reel, type Built, type Shape } from "./reel";

export interface SentinelFilmWords {
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
  talk: Rect;
  machine: Rect;
  /** Where the sentinel stands while it watches. */
  post: Point;
  label: number;
}

const WIDE: Layout = {
  width: 1600,
  height: 900,
  talk: { x: 80, y: 150, w: 420, h: 560 },
  machine: { x: 1100, y: 250, w: 380, h: 330 },
  post: [860, 330],
  label: 26,
};

const TALL: Layout = {
  width: 900,
  height: 1200,
  talk: { x: 60, y: 40, w: 780, h: 380 },
  machine: { x: 260, y: 760, w: 380, h: 330 },
  post: [450, 600],
  label: 32,
};

export function sentinelFilm(words: SentinelFilmWords, shape: Shape): Built {
  const L = shape === "wide" ? WIDE : TALL;
  const reel = new Reel(`sentinel-${shape}`, L.width, L.height);
  const [first, second, third] = words.captions;
  reel.say(200, first);
  reel.say(4300, second);
  reel.say(9300, third);

  const { talk: t, machine: m, post } = L;

  // The conversation rests while the sentinel watches, and comes back.
  reel.group([t.x + t.w / 2, t.y + t.h / 2], [{ t: 0 }, { t: 4300, o: 1 }, { t: 4900, o: 0.45 }, { t: 10300, o: 0.45 }, { t: 10800, o: 1 }], () => {
    reel.wash(t.x, t.y, t.w, t.h, "page", 200, 420, 24);
    reel.mark("claude", [t.x + 26, t.y + 30], 42, "clay", 500);
    reel.scribble([t.x + 86, t.y + 54], t.w - 140, "ink", 650, 520);
    reel.scribble([t.x + 86, t.y + 94], t.w - 200, "ink", 1000, 460);
    // What it waits for, said once.
    reel.wash(t.x + 70, t.y + 140, t.w - 110, 60, "panel", 1500, 300, 14);
    reel.mark("sentinel", [t.x + 86, t.y + 154], 32, "clay", 1700);
    reel.scribble([t.x + 134, t.y + 172], (t.w - 110) * 0.55, "ink", 1850, 420, 3);
  });

  // The machine, its log growing line by line while it is watched.
  reel.wash(m.x, m.y, m.w, m.h, "raised", 2400, 380, 16);
  reel.mark("server", [m.x + 22, m.y + 22], 34, "graphite", 2600);
  reel.text([m.x + m.w / 2, m.y + m.h + L.label * 1.8], "server", L.label, "graphite", 2700, { anchor: "middle" });
  for (let index = 0; index < 6; index += 1) {
    reel.scribble([m.x + 26, m.y + 84 + index * 38], m.w * (index % 2 ? 0.55 : 0.68), "graphite", 3000 + index * 1000, 420, 2.6);
  }
  // The line it waits for: written in clay.
  reel.scribble([m.x + 26, m.y + 84 + 6 * 38], m.w * 0.6, "clay", 9300, 520, 3.2);

  // The sentinel goes to its post beside the machine.
  const from: Point = [t.x + 102, t.y + 170];
  reel.dashes(curvePoints(from, [(from[0] + post[0]) / 2, Math.min(from[1], post[1]) - 80], post), 24, 3, "graphite", 2300, 800);
  reel.group(post, [{ t: 0, o: 0 }, { t: 3000, o: 0 }, { t: 3300, o: 1 }], () => {
    reel.blot(post, 22, "clay", 3000);
    reel.mark("sentinel", [post[0] - 18, post[1] - 18], 36, "ink", 3100);
  });
  // Its look at the log, now and then, without waking anyone.
  const toward: Point = shape === "wide" ? [m.x - 14, m.y + 120] : [m.x + m.w / 2, m.y - 14];
  for (let index = 0; index < 5; index += 1) reel.dashes([post, toward], 16, 2.2, "graphite", 4600 + index * 900, 300);

  // The line comes: it rings, and the conversation wakes.
  reel.ring(post, 38, 3, "clay", 9800, 500);
  reel.dashes(curvePoints(post, [(from[0] + post[0]) / 2, Math.max(from[1], post[1]) + 90], [t.x + t.w - 40, t.y + 240]), 24, 3.2, "clay", 10200, 700);
  reel.scribble([t.x + 86, t.y + 260], t.w - 170, "ink", 11000, 600);
  reel.check([t.x + 34, t.y + 262], 28, "clay", 11700);

  return reel.build(14000);
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
