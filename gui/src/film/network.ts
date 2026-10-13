// The machines page's film, before this computer is in any network
// (`world-gui.md`, 账号、机器、共享与协助). Someone who came for `run` sees
// what a network is for: the agent on this computer works on every machine
// in it, a long job goes on there with the computer closed, files go from
// machine to machine, and a machine can be shared. Twenty-three seconds; the
// same moments wide and tall, only where things stand differs.

import type { Point } from "../pen/shapes";
import { Reel, type Built, type Pose, type Shape } from "./reel";

export interface NetworkWords {
  /** What is said under the film, in order. */
  captions: readonly [string, string, string, string, string];
  /** A thought that is over, as the conversation writes it. */
  thought: string;
  ran: string;
  /** The hub, as the interface names it. */
  hub: string;
  /** The tag on a machine shared with someone. */
  shared: string;
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
  /** This computer's screen as the film opens, close up. */
  screen: Rect;
  /** Where that screen goes when the camera pulls back: onto the laptop. */
  pulled: Pose;
  hub: Point;
  /** The machines: this laptop, a desktop, a box with a GPU. */
  laptop: Point;
  desktop: Point;
  gpu: Point;
  /** Someone the GPU box is shared with. */
  other: Point;
  /** The command the agent writes, beside the laptop's line, and which
   *  end of it stands there. */
  command: Point;
  commandAnchor: "start" | "end";
  /** The bars of the job running beside the GPU box. */
  bars: Point;
  label: number;
}

const WIDE: Layout = {
  width: 1600,
  height: 900,
  screen: { x: 470, y: 150, w: 660, h: 460 },
  pulled: { t: 0, x: -380, y: 210, sx: 0.3, sy: 0.3 },
  hub: [800, 150],
  laptop: [420, 590],
  desktop: [800, 640],
  gpu: [1180, 590],
  other: [1400, 800],
  command: [398, 445],
  commandAnchor: "end",
  bars: [1020, 410],
  label: 26,
};

const TALL: Layout = {
  width: 900,
  height: 1200,
  screen: { x: 90, y: 150, w: 720, h: 560 },
  pulled: { t: 0, x: -230, y: 340, sx: 0.28, sy: 0.28 },
  hub: [450, 150],
  laptop: [220, 770],
  desktop: [450, 1010],
  gpu: [680, 770],
  other: [770, 1110],
  command: [40, 968],
  commandAnchor: "start",
  bars: [560, 560],
  label: 30,
};

export function network(words: NetworkWords, shape: Shape): Built {
  const L = shape === "wide" ? WIDE : TALL;
  const reel = new Reel(`network-${shape}`, L.width, L.height);
  const [first, second, third, fourth, fifth] = words.captions;
  reel.say(200, first);
  reel.say(5200, second);
  reel.say(8000, third);
  reel.say(12300, fourth);
  reel.say(17000, fifth);

  // This computer's screen, close: the agent at work on it.
  const s = L.screen;
  const centre: Point = [s.x + s.w / 2, s.y + s.h / 2];
  const away = { ...L.pulled, t: 4700 };
  // Asleep while the job runs elsewhere, awake again for its answer.
  const laptopPoses: Pose[] = [{ t: 0, o: 1 }, { t: 3800 }, away, { ...away, t: 12500, o: 1 }, { ...away, t: 12900, o: 0.35 }, { ...away, t: 15000, o: 0.35 }, { ...away, t: 15400, o: 1 }];
  reel.group(centre, laptopPoses, () => {
    reel.wash(s.x, s.y, s.w, s.h, "page", 250, 520, 26);
    const left = s.x + 44;
    const right = s.x + s.w - 44;
    reel.mark("codex", [left, s.y + 36], 46, "graphite", 600);
    reel.wash(right - 260, s.y + 40, 260, 60, "bubble", 900, 300, 20);
    reel.scribble([right - 232, s.y + 72], 190, "ink", 1100, 420, 3);
    const row = (n: number) => s.y + 190 + n * 70;
    reel.mark("thinking", [left, row(0) - 26], 30, "graphite", 1600);
    reel.text([left + 48, row(0)], words.thought, 26, "ink", 1660);
    reel.mark("command", [left, row(1) - 26], 30, "graphite", 2100);
    reel.text([left + 48, row(1)], `${words.ran}  cargo test`, 26, "ink", 2160);
    reel.scribble([left, row(2) + 10], 420, "ink", 2700, 700, 3);
    reel.scribble([left, row(2) + 52], 300, "ink", 3200, 500, 3);
  });

  // The laptop it is, under the screen pulled back.
  const [lx, ly] = L.laptop;
  const half = (s.w * L.pulled.sx!) / 2 + 22;
  const under = ly + (s.h * L.pulled.sy!) / 2 + 14;
  const laptopBase = (draw: () => void) => reel.group([lx, under], laptopPoses.slice(2).map((pose) => ({ t: pose.t, o: pose.o ?? 1 })), draw);
  laptopBase(() => {
    reel.line([lx - half, under], [lx + half, under], 6, "graphite", 4800, 260);
    reel.text([lx, under + 46], "laptop", L.label, "graphite", 5000, { anchor: "middle" });
  });

  // The hub, and the machines that reach it.
  const [hx, hy] = L.hub;
  reel.ring([hx, hy], 26, 3.2, "ink", 5300, 420);
  reel.blot([hx, hy], 7, "ink", 5700);
  reel.text([hx, hy - 50], words.hub, L.label, "ink", 5800, { anchor: "middle" });
  const machine = (at: Point, name: "desktop" | "server", label: string, when: number) => {
    reel.mark(name, [at[0] - 56, at[1] - 56], 112, "graphite", when);
    reel.text([at[0], at[1] + 92], label, L.label, "graphite", when + 150, { anchor: "middle" });
  };
  machine(L.desktop, "desktop", "desktop", 6000);
  machine(L.gpu, "server", "gpu", 6250);
  const toHub = (from: Point): Point[] => square(from, [hx, hy + 32]);
  const laptopLine = toHub([lx, ly - 80]);
  const desktopLine = toHub([L.desktop[0], L.desktop[1] - 70]);
  const gpuLine = toHub([L.gpu[0], L.gpu[1] - 70]);
  reel.dashes(laptopLine, 22, 3.4, "graphite", 6600, 700);
  reel.dashes(desktopLine, 22, 3.4, "graphite", 6800, 500);
  reel.dashes(gpuLine, 22, 3.4, "graphite", 7000, 700);

  // The agent sends a command to the GPU box: it goes up to the hub and
  // down again, and the job runs there.
  reel.text(L.command, "on gpu  python train.py", shape === "wide" ? 24 : 22, "ink", 8200, { anchor: L.commandAnchor, mono: true });
  const out = [...laptopLine, ...[...gpuLine].reverse()];
  for (let index = 0; index < 3; index += 1) reel.travel(out, 9, "clay", 8700 + index * 380, 1300);
  const [bx, by] = L.bars;
  const bar = (n: number, at: number, dur: number) => {
    reel.wash(bx, by + n * 34, 110, 20, "panel", at - 200, 200, 8);
    reel.wash(bx, by + n * 34, 110, 20, "clay", at, dur, 8);
  };
  bar(0, 10200, 1500);
  bar(1, 11400, 2200);
  bar(2, 13300, 1300);
  // A moon over the laptop while it sleeps.
  reel.group([lx, ly - 128], [{ t: 12800, o: 0 }, { t: 13100, o: 1 }, { t: 15000, o: 1 }, { t: 15400, o: 0 }], () => reel.mark("sleep", [lx - 22, ly - 150], 44, "graphite", 12800));
  reel.check([L.gpu[0] + 74, L.gpu[1] - 40], 30, "clay", 14800);
  // The answer waits, and comes back when the laptop wakes.
  const back = [...gpuLine, ...[...laptopLine].reverse()];
  reel.travel(back, 10, "clay", 15400, 1300);
  reel.check([lx + half + 16, ly - 20], 30, "clay", 16700);

  // A file goes from the GPU box to the desktop, straight across.
  const [dx, dy] = L.desktop;
  const [gx, gy] = L.gpu;
  const fileFrom: Point = [gx - 20, gy + 40];
  const fileTo: Point = [dx + 30, dy + 10];
  reel.group(fileFrom, [{ t: 17300, o: 0, sx: 0.5, sy: 0.5 }, { t: 17600, o: 1, sx: 1, sy: 1 }, { t: 18900, x: fileTo[0] - fileFrom[0], y: fileTo[1] - fileFrom[1] }, { t: 19200, o: 0 }], () =>
    reel.mark("file", [fileFrom[0] - 22, fileFrom[1] - 22], 44, "clay", 17300),
  );
  reel.dashes(arc(fileFrom, fileTo, shape === "wide" ? 90 : 70), 20, 3, "clay", 17500, 1300);

  // Someone else, and the GPU box shared with them.
  const [ox, oy] = L.other;
  reel.mark("person", [ox - 36, oy - 36], 72, "graphite", 19600);
  reel.dashes(arc([ox - 30, oy - 30], [gx + 40, gy + 50], -40), 20, 3, "graphite", 19900, 700);
  reel.text([gx + 70, gy + 116], words.shared, L.label - 4, "clay", 20600);

  return reel.build(23000);
}

/** A square path, as the machines page draws its lines: up from `from`,
 *  across, then up into `to`. */
function square(from: Point, to: Point): Point[] {
  const turn = from[1] - (from[1] - to[1]) * 0.55;
  return [from, [from[0], turn], [to[0], turn], to];
}

/** Points along a gentle arc between two points, bent by `bend`. */
function arc(from: Point, to: Point, bend: number): Point[] {
  const mid: Point = [(from[0] + to[0]) / 2, (from[1] + to[1]) / 2 + bend];
  const points: Point[] = [];
  for (let index = 0; index <= 30; index += 1) {
    const t = index / 30;
    const v = 1 - t;
    points.push([v * v * from[0] + 2 * v * t * mid[0] + t * t * to[0], v * v * from[1] + 2 * v * t * mid[1] + t * t * to[1]]);
  }
  return points;
}
