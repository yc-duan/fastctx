// History's film (`world-history.md`). A conversation writes down what
// happened, what was decided and what was let go, in a document of plain
// lines; the next day another vendor's model wakes to the same work,
// searches, finds the line that settles it and carries on, adding its own;
// and the document is a plain file on every computer. Fifteen seconds,
// wide and tall.

import type { Point } from "../pen/shapes";
import { Reel, type Built, type Shape } from "./reel";

export interface HistoryFilmWords {
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
  /** Where the conversations stand, one after the other. */
  talk: Rect;
  /** The document. */
  doc: Rect;
  /** Its lines: the first one's baseline and the step to the next. */
  first: number;
  step: number;
  /** The computers it is copied to, and where each one's copy lies. */
  machines: { mark: "desktop" | "laptop" | "server"; at: Point; sheet: Point }[];
  /** The sun's way across, as a day goes by. */
  day: [Point, Point, Point];
  label: number;
}

const WIDE: Layout = {
  width: 1600,
  height: 900,
  talk: { x: 70, y: 200, w: 430, h: 480 },
  doc: { x: 600, y: 120, w: 560, h: 660 },
  first: 270,
  step: 92,
  machines: [
    { mark: "desktop", at: [1300, 170], sheet: [1390, 168] },
    { mark: "laptop", at: [1300, 400], sheet: [1390, 398] },
    { mark: "server", at: [1300, 630], sheet: [1390, 628] },
  ],
  day: [
    [90, 120],
    [290, 20],
    [490, 120],
  ],
  label: 26,
};

const TALL: Layout = {
  width: 900,
  height: 1200,
  talk: { x: 60, y: 90, w: 780, h: 290 },
  doc: { x: 100, y: 430, w: 700, h: 520 },
  first: 584,
  step: 78,
  machines: [
    { mark: "desktop", at: [150, 1010], sheet: [212, 1008] },
    { mark: "laptop", at: [390, 1010], sheet: [452, 1008] },
    { mark: "server", at: [630, 1010], sheet: [692, 1008] },
  ],
  day: [
    [80, 70],
    [450, 0],
    [820, 70],
  ],
  label: 32,
};

/** The document's lines, by day: the decision found again is the second;
 *  the third is crossed out once the fourth corrects it. */
const DAYS = ["09-25", "09-30", "10-02", "10-04"] as const;
const FOUND = 1;
const CROSSED = 2;

export function historyFilm(words: HistoryFilmWords, shape: Shape): Built {
  const L = shape === "wide" ? WIDE : TALL;
  const reel = new Reel(`history-${shape}`, L.width, L.height);
  const [first, second, third, fourth] = words.captions;
  reel.say(200, first);
  reel.say(4600, second);
  reel.say(8000, third);
  reel.say(12400, fourth);

  const { talk: t, doc: d } = L;
  const wide = shape === "wide";
  const line = (index: number) => L.first + index * L.step;
  const dateWidth = L.label * 3.4;
  const lineFrom = d.x + 40 + dateWidth;
  const lineLength = (index: number) => (d.w - 90 - dateWidth) * (index % 2 ? 0.78 : 0.92);

  // The document: its name, then a dated line for each thing that happened.
  reel.wash(d.x, d.y, d.w, d.h, "raised", 1300, 420, 18);
  // The line found again glows under its words, so it is laid before them.
  reel.group([d.x, line(FOUND)], [{ t: 0, o: 0 }, { t: 9300, o: 0 }, { t: 9700, o: 0.55 }], () =>
    reel.wash(d.x + 22, line(FOUND) - L.label * 1.15, d.w - 44, L.label * 1.75, "clay", 9300, 420, 12),
  );
  reel.mark("journal", [d.x + 30, d.y + 34], 34, "graphite", 1500);
  reel.text([d.x + 80, d.y + 62], "q16-fixed-point.md", L.label, "ink", 1600, { mono: true });
  reel.line([d.x + 30, d.y + 92], [d.x + d.w - 30, d.y + 92], 2.2, "graphite", 1800, 360);
  DAYS.forEach((day, index) => {
    const at = 2400 + index * 520;
    reel.text([d.x + 34, line(index)], day, L.label * 0.9, "graphite", at, { mono: true });
    reel.scribble([lineFrom, line(index) - L.label * 0.3], lineLength(index), "ink", at + 120, 420, 3);
  });
  // What was let go stays, crossed out.
  reel.line([lineFrom - 6, line(CROSSED) - L.label * 0.32], [lineFrom + lineLength(CROSSED) + 6, line(CROSSED) - L.label * 0.38], 2.6, "graphite", 4200, 300);

  // Today's conversation: asked, it answers and writes it down, then rests.
  const reply = wide ? 130 : 108;
  reel.group([t.x + t.w / 2, t.y + t.h / 2], [{ t: 0 }, { t: 4600, o: 1 }, { t: 5200, o: 0 }], () => {
    reel.wash(t.x, t.y, t.w, t.h, "page", 200, 420, 24);
    reel.wash(t.x + t.w - 270, t.y + 28, 240, 56, "bubble", 400, 260, 18);
    reel.scribble([t.x + t.w - 248, t.y + 56], 180, "ink", 560, 380);
    reel.mark("claude", [t.x + 26, t.y + reply - 20], 42, "clay", 900);
    reel.scribble([t.x + 86, t.y + reply + 4], t.w - 140, "ink", 1050, 520);
    reel.scribble([t.x + 86, t.y + reply + 44], t.w - 200, "ink", 1400, 460);
  });
  const from: Point = wide ? [t.x + t.w - 20, t.y + reply + 40] : [t.x + t.w / 2, t.y + t.h - 16];
  const into: Point = wide ? [d.x - 14, line(0) - 10] : [d.x + d.w / 2, d.y - 14];
  reel.dashes(bend(from, wide ? [(from[0] + into[0]) / 2, from[1] - 30] : [from[0] + 120, (from[1] + into[1]) / 2], into), 28, 3.8, "graphite", 1900, 600);

  // A day goes by.
  reel.travel(bend(...L.day), 20, "clay", 4700, 1300);

  // The next day another model wakes to the same work, and searches.
  reel.group([t.x + t.w / 2, t.y + t.h / 2], [{ t: 0, o: 0 }, { t: 5800, o: 0 }, { t: 6200, o: 1 }], () => {
    reel.wash(t.x, t.y, t.w, t.h, "page", 5800, 420, 24);
    reel.mark("codex", [t.x + 26, t.y + 30], 42, "graphite", 6100);
    reel.wash(t.x + t.w - 270, t.y + 30, 240, 56, "bubble", 6400, 260, 18);
    reel.scribble([t.x + t.w - 248, t.y + 58], 180, "ink", 6600, 380);
    const bar = wide ? { y: t.y + 120, h: 58 } : { y: t.y + 108, h: 58 };
    reel.wash(t.x + 70, bar.y, t.w - 100, bar.h, "panel", 8000, 300, 12);
    reel.text([t.x + 92, bar.y + bar.h * 0.66], "rg Q16 history/", L.label * 0.95, "ink", 8200, { mono: true });
    // It goes on from what it found, and checks the step off.
    reel.scribble([t.x + 86, bar.y + bar.h + 56], t.w - 170, "ink", 10200, 560);
    reel.scribble([t.x + 86, bar.y + bar.h + 96], t.w - 230, "ink", 10700, 480);
    reel.check([t.x + 34, bar.y + bar.h + 58], 28, "clay", 11300);
  });
  // Wide, the search reaches the line from its left; tall, down the margin.
  const asked: Point = wide ? [t.x + t.w - 30, t.y + 150] : [t.x + t.w - 40, t.y + 230];
  const found: Point = wide ? [d.x + 16, line(FOUND) - L.label * 0.3] : [d.x + d.w - 20, line(FOUND) - L.label * 0.3];
  reel.dashes(bend(asked, wide ? [(asked[0] + found[0]) / 2, found[1] - 60] : [asked[0] + 70, (asked[1] + found[1]) / 2], found), 28, 3.8, "clay", 8800, 600);
  // And adds what it settled, a line of its own.
  reel.text([d.x + 34, line(DAYS.length)], "10-08", L.label * 0.9, "graphite", 11500, { mono: true });
  reel.scribble([lineFrom, line(DAYS.length) - L.label * 0.3], lineLength(0) * 0.85, "clay", 11620, 520, 3);

  // A plain file on every computer, read by any vendor's model.
  L.machines.forEach(({ mark, at, sheet }, index) => {
    const when = 12600 + index * 380;
    reel.mark(mark, at, 56, "graphite", when);
    reel.text([at[0] + 28, at[1] + 56 + L.label * 1.3], mark, L.label * 0.85, "graphite", when + 120, { anchor: "middle" });
    reel.wash(sheet[0], sheet[1], 74, 92, "raised", when + 400, 260, 8);
    for (let row = 0; row < 3; row += 1) reel.scribble([sheet[0] + 12, sheet[1] + 26 + row * 22], 46 - (row % 2) * 12, "graphite", when + 560 + row * 120, 160, 2);
    const leaving: Point = wide ? [d.x + d.w + 12, d.y + d.h / 2] : [d.x + d.w / 2, d.y + d.h + 12];
    const arriving: Point = wide ? [at[0] - 12, at[1] + 28] : [at[0] + 28, at[1] - 12];
    reel.dashes([leaving, arriving], 22, 3, "graphite", when + 100, 420);
  });

  return reel.build(15500);
}

/** A curve from one point to another, bent toward `through`, its points
 *  far enough apart that a dash drawn along it is not cut short. */
function bend(from: Point, through: Point, to: Point): Point[] {
  const reach = Math.hypot(through[0] - from[0], through[1] - from[1]) + Math.hypot(to[0] - through[0], to[1] - through[1]);
  const count = Math.max(2, Math.round(reach / 40));
  const points: Point[] = [];
  for (let index = 0; index <= count; index += 1) {
    const t = index / count;
    const v = 1 - t;
    points.push([v * v * from[0] + 2 * v * t * through[0] + t * t * to[0], v * v * from[1] + 2 * v * t * through[1] + t * t * to[1]]);
  }
  return points;
}
