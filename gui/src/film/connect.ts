// The film of what connecting is (`world-gui.md`, 接入): the user goes on
// working in their own agent; connected, it has one more tool, run, that
// reaches each of their machines through Fastctx; and what it does there is
// written down in the activity. Ten seconds, the same moments wide and
// tall; only where things stand differs.

import type { GlyphName } from "../conversation/glyphs";
import { quadratic, type Point } from "../pen/shapes";
import { Reel, type Built, type Shape } from "./reel";

export interface ConnectFilmWords {
  /** What is said under the film, in order. */
  captions: readonly [string, string, string];
  /** The activity page's name, as the sidebar writes it. */
  activity: string;
}

interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}

interface Machine {
  name: string;
  kind: "desktop" | "laptop" | "server";
  /** Its screen, or a server's body. */
  body: Rect;
  /** Where the line from the agent comes to it, and where it bends. */
  reach: Point;
  bend: Point;
  label: Point;
}

/** Where everything stands, for one shape of the film. */
interface Layout {
  width: number;
  height: number;
  window: Rect;
  /** Where run's mark stands, where its line leaves, and its name. */
  plug: Point;
  out: Point;
  run: Point;
  runAnchor: "start" | "middle";
  machines: [Machine, Machine, Machine];
  ledger: Rect;
}

const WIDE: Layout = {
  width: 1600,
  height: 820,
  window: { x: 90, y: 90, w: 580, h: 410 },
  plug: [690, 268],
  out: [754, 296],
  run: [718, 384],
  runAnchor: "middle",
  machines: [
    { name: "desktop", kind: "desktop", body: { x: 1100, y: 60, w: 260, h: 160 }, reach: [1086, 140], bend: [900, 150], label: [1230, 314] },
    { name: "laptop", kind: "laptop", body: { x: 1230, y: 330, w: 240, h: 148 }, reach: [1216, 404], bend: [980, 350], label: [1350, 548] },
    { name: "server", kind: "server", body: { x: 1110, y: 556, w: 170, h: 180 }, reach: [1096, 646], bend: [900, 570], label: [1195, 790] },
  ],
  ledger: { x: 90, y: 560, w: 580, h: 220 },
};

const TALL: Layout = {
  width: 900,
  height: 1120,
  window: { x: 50, y: 40, w: 800, h: 340 },
  plug: [418, 392],
  out: [446, 452],
  run: [494, 434],
  runAnchor: "start",
  machines: [
    { name: "desktop", kind: "desktop", body: { x: 60, y: 540, w: 210, h: 136 }, reach: [165, 530], bend: [250, 470], label: [165, 766] },
    { name: "laptop", kind: "laptop", body: { x: 345, y: 550, w: 210, h: 126 }, reach: [450, 540], bend: [450, 490], label: [450, 766] },
    { name: "server", kind: "server", body: { x: 650, y: 530, w: 170, h: 180 }, reach: [735, 520], bend: [650, 470], label: [735, 766] },
  ],
  ledger: { x: 50, y: 806, w: 800, h: 290 },
};

/** The film for `mark`'s agent, in `shape`. */
export function connect(words: ConnectFilmWords, mark: GlyphName, shape: Shape): Built {
  const L = shape === "wide" ? WIDE : TALL;
  const reel = new Reel(`connect-${shape}`, L.width, L.height);
  const [first, second, third] = words.captions;
  reel.say(200, first);
  reel.say(3000, second);
  reel.say(6000, third);

  // The agent's own window, where the user goes on working.
  const W = L.window;
  reel.wash(W.x, W.y, W.w, W.h, "raised", 200, 520, 24);
  reel.wash(W.x + 20, W.y + W.h - 76, W.w - 40, 54, "panel", 520, 360, 18);
  reel.mark(mark, [W.x + 26, W.y + 22], 48, "graphite", 700);
  reel.scribble([W.x + 92, W.y + 48], 150, "graphite", 780, 360, 2.6);
  const right = W.x + W.w - 30;
  reel.wash(right - 270, W.y + 96, 270, 56, "bubble", 1100, 300, 20);
  reel.scribble([right - 246, W.y + 125], 210, "ink", 1300, 420, 3);
  reel.scribble([W.x + 30, W.y + 196], Math.min(430, W.w - 110), "ink", 1900, 520, 3);
  reel.scribble([W.x + 30, W.y + 238], 320, "ink", 2400, 420, 3);

  // Connected: one more tool, run, and a line from it to each machine.
  reel.mark("plug", L.plug, 56, "clay", 3100);
  reel.text(L.run, "run", 40, "ink", 3250, { anchor: L.runAnchor, mono: true });
  L.machines.forEach((machine, index) => drawMachine(reel, machine, 3500 + index * 200));
  const lines = L.machines.map((machine) => quadratic(L.out, machine.bend, machine.reach, 6));
  lines.forEach((line, index) => reel.pen(line, 2.4, "ink", 4500 + index * 300, 650));

  // What it does there runs along the lines, and the activity writes each
  // down as it comes.
  const D = L.ledger;
  reel.wash(D.x, D.y, D.w, D.h, "page", 6000, 420, 22);
  reel.mark("history", [D.x + 28, D.y + 24], 36, "graphite", 6250);
  reel.text([D.x + 76, D.y + 54], words.activity, 36, "ink", 6320);
  const row = (n: number) => D.y + 108 + n * (shape === "wide" ? 42 : 58);
  L.machines.forEach((machine, index) => {
    const leaves = 6500 + index * 500;
    const lands = leaves + 900;
    reel.travel(lines[index]!, 9, "clay", leaves, 900);
    const b = machine.body;
    const spot: Point = machine.kind === "server" ? [b.x + b.w / 2 - 24, b.y + b.h - 78] : [b.x + b.w / 2 - 24, b.y + b.h / 2 - 24];
    reel.mark("command", spot, 48, "graphite", lands);
    const y = row(index);
    reel.mark("command", [D.x + 30, y - 22], 28, "graphite", lands + 50);
    reel.scribble([D.x + 74, y - 8], D.w * 0.55 - index * 30, "graphite", lands + 100, 360, 2.4);
    reel.scribble([D.x + D.w - 112, y - 8], 72, "graphite", lands + 300, 200, 2.2);
  });

  return reel.build(9600);
}

/** A machine drawn in as the drawing names it: a screen on a stand, a
 *  screen on its base, or a server's body. */
function drawMachine(reel: Reel, machine: Machine, at: number): void {
  const b = machine.body;
  const middle = b.x + b.w / 2;
  reel.wash(b.x, b.y, b.w, b.h, "raised", at, 380, machine.kind === "server" ? 12 : 14);
  if (machine.kind === "desktop") {
    reel.line([middle, b.y + b.h + 4], [middle, b.y + b.h + 30], 5, "graphite", at + 220, 200);
    reel.line([middle - 50, b.y + b.h + 36], [middle + 50, b.y + b.h + 36], 5, "graphite", at + 340, 240);
  } else if (machine.kind === "laptop") {
    reel.wash(b.x - 30, b.y + b.h - 2, b.w + 60, 20, "panel", at + 220, 300, 10);
  } else {
    for (let index = 0; index < 2; index += 1) reel.line([b.x + 22, b.y + 32 + index * 24], [b.x + b.w - 22, b.y + 32 + index * 24], 3, "graphite", at + 200 + index * 100, 200);
  }
  reel.text(machine.label, machine.name, 36, "graphite", at + 400, { anchor: "middle" });
}
