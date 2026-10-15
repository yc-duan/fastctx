// The workbench's film (`world-gui.md`, 功能). Elsewhere, what an agent does
// through `run` reaches the user as a wall of text; the workbench draws it
// step by step. An agent on another computer opens here as if it were this
// one, and the same conversation goes on in a phone. Seventeen seconds, the
// same moments wide and tall; only where things stand differs.

import type { Point } from "../pen/shapes";
import { Reel, type Built, type Pose, type Shape } from "./reel";

export interface WorkbenchWords {
  /** What is said under the film, in order. */
  captions: readonly [string, string, string, string];
  /** A thought that is over, as the conversation writes it. */
  thought: string;
  ran: string;
  edited: string;
  /** The card a phone shows when the agent waits on the user. */
  approve: string;
}

interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}

/** Where everything stands, for one shape of the film. */
interface Layout {
  width: number;
  height: number;
  /** Elsewhere: the raw text a tool call is shown as. */
  dry: Rect;
  dryType: number;
  dryLines: number;
  /** The workbench, and how wide its sidebar is. */
  window: Rect;
  side: number;
  /** The size of everything inside the window, against the wide film. */
  u: number;
  /** Where the window goes when the camera pulls back. */
  pulled: Pose;
  /** This computer's stand, under the window pulled back. */
  stand: { top: Point; base: number; label: Point };
  /** The other computer: its screen. */
  remote: Rect;
  /** Where the other computer moves to when the phone comes. */
  remoteMoves: Pose | null;
  phone: Rect;
  label: number;
}

const WIDE: Layout = {
  width: 1600,
  height: 900,
  dry: { x: 110, y: 200, w: 560, h: 440 },
  dryType: 21,
  dryLines: 9,
  window: { x: 700, y: 110, w: 780, h: 680 },
  side: 150,
  u: 1,
  pulled: { t: 0, x: -540, y: -50, sx: 0.6, sy: 0.6 },
  stand: { top: [550, 610], base: 76, label: [550, 728] },
  remote: { x: 1150, y: 280, w: 280, h: 190 },
  remoteMoves: null,
  phone: { x: 860, y: 520, w: 160, h: 310 },
  label: 26,
};

const TALL: Layout = {
  width: 900,
  height: 1200,
  dry: { x: 50, y: 40, w: 800, h: 300 },
  dryType: 23,
  dryLines: 6,
  window: { x: 60, y: 372, w: 780, h: 816 },
  side: 0,
  u: 1.25,
  pulled: { t: 0, x: 0, y: -500, sx: 0.55, sy: 0.55 },
  stand: { top: [450, 512], base: 70, label: [450, 628] },
  remote: { x: 310, y: 720, w: 280, h: 190 },
  remoteMoves: { t: 0, x: -170, y: 0 },
  phone: { x: 600, y: 650, w: 190, h: 360 },
  label: 32,
};

/** What a tool call looks like where the host shows only its text. */
const DRY = [
  'fastctx.run({"script":"cd C:/work/mark50 && cargo test"})',
  "=== run on laptop (exited 0) ===",
  "running 42 tests",
  "test ctrl::step_response ... ok",
  "test ctrl::fixed::q16 ... ok",
  "test result: ok. 42 passed; 0 failed",
  'fastctx.run({"script":"edit C:/work/mark50/src/pid.rs <<\'EOF\'…"})',
  "=== edit C:/work/mark50/src/pid.rs on laptop (1 change) ===",
  "41\t    pub fn step(&mut self, error: Q16, dt: Q16) -> Q16 {",
];

export function workbench(words: WorkbenchWords, shape: Shape): Built {
  const L = shape === "wide" ? WIDE : TALL;
  const reel = new Reel(`workbench-${shape}`, L.width, L.height);
  const u = L.u;
  const [first, second, third, fourth] = words.captions;
  reel.say(200, first);
  reel.say(2900, second);
  reel.say(6300, third);
  reel.say(11700, fourth);

  // Elsewhere: the call and what came back, as text and nothing else.
  const { dry } = L;
  // As many characters as the box holds inside its margins: the program's
  // type is monospaced, each character six tenths of its size wide.
  const fits = Math.floor((dry.w - 52) / (L.dryType * 0.6));
  // Tall, the text alone stands in the middle of the frame, and rises to
  // the top before the workbench draws in beneath it.
  const lift = L.height / 2 - (dry.y + dry.h / 2);
  const fading: Pose[] =
    shape === "wide"
      ? [{ t: 0 }, { t: 2600 }, { t: 3500, x: -40, sx: 0.95, sy: 0.95, o: 0.4 }, { t: 5600, o: 0.4 }, { t: 6400, o: 0 }]
      : [{ t: 0, y: lift }, { t: 2050 }, { t: 2650, y: 0 }, { t: 3500, sx: 0.95, sy: 0.95, o: 0.4 }, { t: 5600 }, { t: 6400, o: 0 }];
  reel.group([dry.x + dry.w / 2, dry.y + dry.h / 2], fading, () => {
    reel.wash(dry.x, dry.y, dry.w, dry.h, "raised", 250, 380, 18);
    DRY.slice(0, L.dryLines).forEach((line, index) => {
      reel.text([dry.x + 26, dry.y + 50 + index * (L.dryType * 1.75)], clip(line, fits), L.dryType, "graphite", 450 + index * 130, { mono: true });
    });
  });

  // The workbench: the same steps, drawn.
  const { window: w } = L;
  const centre: Point = [w.x + w.w / 2, w.y + w.h / 2];
  const pulled = L.pulled;
  reel.group(centre, [{ t: 0 }, { t: 6200 }, { ...pulled, t: 7300 }], () => {
    reel.wash(w.x, w.y, w.w, w.h, "page", 2700, 520, 26);
    if (L.side) reel.wash(w.x, w.y, L.side, w.h, "panel", 2850, 360, 26);
    for (let index = 0; index < 4 && L.side; index += 1) reel.scribble([w.x + 28, w.y + 70 + index * 46], 60 + ((index * 23) % 40), "graphite", 3000 + index * 90, 220, 2.6);
    const left = w.x + L.side + 34 * u;
    const right = w.x + w.w - 34 * u;
    // The user's words.
    reel.wash(right - 240 * u, w.y + 36 * u, 240 * u, 58 * u, "bubble", 3150, 300, 20 * u);
    reel.scribble([right - 214 * u, w.y + 66 * u], 170 * u, "ink", 3350, 380, 3 * u);
    // Its steps, a row each.
    const row = (n: number) => w.y + 170 * u + n * 64 * u;
    const type = 26 * u;
    reel.mark("thinking", [left, row(0) - 24 * u], 30 * u, "graphite", 3700);
    reel.text([left + 46 * u, row(0)], words.thought, type, "ink", 3760);
    reel.mark("command", [left, row(1) - 24 * u], 30 * u, "graphite", 4150);
    reel.text([left + 46 * u, row(1)], `${words.ran}  cargo test`, type, "ink", 4210);
    reel.text([left + 390 * u, row(1)], "laptop", 22 * u, "graphite", 4330);
    reel.mark("edit", [left, row(2) - 24 * u], 30 * u, "graphite", 4600);
    reel.text([left + 46 * u, row(2)], `${words.edited}  pid.rs`, type, "ink", 4660);
    reel.text([left + 250 * u, row(2)], "+12", type, "add", 4780);
    reel.text([left + 312 * u, row(2)], "−3", type, "del", 4840);
    // The change, a line a row, washed in the colours of what went and came.
    const band = (n: number) => row(2) + 26 * u + n * 34 * u;
    const bands: ["add" | "del", number][] = [
      ["del", 300],
      ["add", 350],
      ["add", 270],
    ];
    bands.forEach(([tone, length], n) => {
      reel.wash(left + 46 * u, band(n), 470 * u, 28 * u, tone, 5000 + n * 160, 300, 6 * u);
      reel.scribble([left + 66 * u, band(n) + 15 * u], length * u, "ink", 5080 + n * 160, 340, 2.4 * u);
    });
    // Then what it says, and, once the camera pulls back, more steps.
    const said = band(3) + 46 * u;
    reel.scribble([left, said], 470 * u, "ink", 5500, 700, 3 * u);
    reel.scribble([left, said + 40 * u], 360 * u, "ink", 6100, 600, 3 * u);
    reel.mark("command", [left, said + 84 * u], 30 * u, "graphite", 9000);
    reel.scribble([left + 46 * u, said + 102 * u], 300 * u, "graphite", 9060, 500, 2.6 * u);
    reel.mark("search", [left, said + 140 * u], 30 * u, "graphite", 10100);
    reel.scribble([left + 46 * u, said + 158 * u], 240 * u, "graphite", 10160, 500, 2.6 * u);
  });

  // This computer: the window is its screen.
  const [sx, sy] = L.stand.top;
  reel.line([sx, sy], [sx, sy + 44], 5, "graphite", 7400, 220);
  reel.line([sx - L.stand.base, sy + 50], [sx + L.stand.base, sy + 50], 5, "graphite", 7550, 260);
  reel.text(L.stand.label, "desktop", L.label, "graphite", 7700, { anchor: "middle" });

  // The other computer, where the agent is, and the line between.
  const r = L.remote;
  const moves = L.remoteMoves;
  const remoteCentre: Point = [r.x + r.w / 2, r.y + r.h / 2];
  const away = moves ? [{ t: 0 }, { t: 11300 }, { ...moves, t: 12100 }] : [{ t: 0 }];
  reel.group(remoteCentre, away, () => {
    reel.wash(r.x, r.y, r.w, r.h, "raised", 7700, 380, 16);
    reel.wash(r.x - 40, r.y + r.h - 2, r.w + 80, 24, "panel", 7900, 300, 12);
    reel.mark("codex", [remoteCentre[0] - 42, remoteCentre[1] - 46], 84, "graphite", 8150);
    reel.text([remoteCentre[0], r.y + r.h + 72], "laptop", L.label, "graphite", 8300, { anchor: "middle" });
  });
  // The line comes to the right of this computer's screen.
  const here: Point = [centre[0] + pulled.x! + (w.w * pulled.sx!) / 2 + 14, centre[1] + pulled.y!];
  const from: Point = shape === "wide" ? [r.x - 18, remoteCentre[1] - 10] : [r.x + r.w - 30, r.y - 20];
  const bend: Point = shape === "wide" ? [(from[0] + here[0]) / 2, from[1] - 110] : [here[0] + 110, (from[1] + here[1]) / 2];
  const link = curve(from, bend, here);
  reel.dashes(link, 26, 4, "graphite", 8450, 700);
  for (let index = 0; index < 5; index += 1) reel.travel(link, 8, "clay", 9100 + index * 520, 1100);

  // A phone joins the same line; the agent asks, and is answered there.
  const p = L.phone;
  const pad = shape === "wide" ? 12 : 14;
  reel.wash(p.x, p.y, p.w, p.h, "raised", 11800, 360, 30);
  reel.wash(p.x + pad, p.y + pad + 18, p.w - pad * 2, p.h - pad * 2 - 36, "page", 11980, 300, 16);
  const remoteNow: Point = moves ? [remoteCentre[0] + moves.x!, remoteCentre[1] + moves.y!] : remoteCentre;
  const toPhone: Point[] =
    shape === "wide"
      ? curve([r.x - 30, r.y + r.h + 26], [p.x + p.w + 150, p.y + 70], [p.x + p.w + 16, p.y + p.h / 2])
      : curve([remoteNow[0] + r.w / 2 + 16, remoteNow[1]], [p.x - 40, remoteNow[1] - 60], [p.x - 14, p.y + p.h / 2]);
  reel.dashes(toPhone, 24, 4, "graphite", 12200, 600);
  for (let index = 0; index < 4; index += 1) reel.travel(toPhone, 7, "clay", 12700 + index * 600, 1000);
  const inner = p.x + pad + 14;
  const span = p.w - pad * 2 - 28;
  for (let index = 0; index < 4; index += 1) {
    const y = p.y + 120 + index * (p.h / 9);
    reel.mark(index === 0 ? "thinking" : index === 3 ? "edit" : "command", [inner, y - 14], 20, "graphite", 12500 + index * 220);
    reel.scribble([inner + 28, y - 4], span - 40 - ((index * 17) % 30), "graphite", 12560 + index * 220, 260, 2.4);
  }
  // The card it asks with drops in at the top of the phone's screen.
  const card: Rect = { x: p.x + pad + 6, y: p.y + pad + 26, w: p.w - pad * 2 - 12, h: shape === "wide" ? 52 : 62 };
  reel.group([card.x + card.w / 2, card.y + card.h / 2], [{ t: 13500, y: -40, o: 0 }, { t: 13950, y: 0, o: 1 }], () => {
    reel.wash(card.x, card.y, card.w, card.h, "clay", 13500, 200, 12);
    reel.text([card.x + card.w / 2, card.y + card.h / 2 + 7], words.approve, shape === "wide" ? 18 : 22, "ink", 13700, { anchor: "middle" });
  });
  // A finger answers: a ring spreads from where it touched, then a tick.
  const touch: Point = [card.x + card.w / 2, card.y + card.h / 2];
  reel.group(touch, [{ t: 14700, sx: 0.3, sy: 0.3, o: 0 }, { t: 14760, o: 1 }, { t: 15400, sx: 1.6, sy: 1.6, o: 0 }], () => reel.ring(touch, 30, 3, "clay", 14700, 200));
  reel.check([card.x + card.w + (shape === "wide" ? 16 : 10), card.y + card.h / 2], shape === "wide" ? 26 : 30, "clay", 15400);

  return reel.build(17000);
}

/** Points along a gentle curve, a few units apart. */
function curve(from: Point, bend: Point, to: Point): Point[] {
  const points: Point[] = [];
  for (let index = 0; index <= 40; index += 1) {
    const t = index / 40;
    const v = 1 - t;
    points.push([v * v * from[0] + 2 * v * t * bend[0] + t * t * to[0], v * v * from[1] + 2 * v * t * bend[1] + t * t * to[1]]);
  }
  return points;
}

/** A line cut to `max` characters, as a narrow box cuts it. */
function clip(line: string, max: number): string {
  return line.length > max ? `${line.slice(0, max - 1)}…` : line;
}
