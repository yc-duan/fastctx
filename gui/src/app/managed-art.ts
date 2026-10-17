// The drawings of bringing a device into a network: the three steps of
// each way in (another computer, a phone, a Linux host by its line, a
// managed machine by its code, `world-network.md`, 受管机器), and the managed
// machine itself, one line to its server and others' lines coming in.
// Drawn in the film's hand (`../pen/sketch`): screens and phones are washes,
// stands and joins are fine ink, typed words a line of handwriting, and the
// accent one or two marks. The words beside them stay text.

import type { Point } from "../pen/shapes";
import { Sketch } from "../pen/sketch";

const STEP = "managed-step-art";

/** A computer: its screen a wash, `w` by `h` from its top left corner, its
 *  stand two ruled strokes. */
function computer(sheet: Sketch, x = 14, y = 6, w = 92, h = 58): Sketch {
  const middle = x + w / 2;
  return sheet
    .wash(x, y, w, h, "box", 7)
    .rule([middle, y + h + 0.5], [middle, y + h + 9], 1.7, "graphite")
    .rule([middle - 14, y + h + 10], [middle + 14, y + h + 10], 2, "graphite");
}

/** A phone standing, `w` by `h` from its top left corner. */
function phone(sheet: Sketch, x = 42, y = 4, w = 36, h = 72): Sketch {
  return sheet.wash(x, y, w, h, "box", 8).rule([x + w / 2 - 5, y + h - 6], [x + w / 2 + 5, y + h - 6], 1.6, "graphite");
}

/** Fastctx: a few machines joined by fine ink around one, the clay one. */
function network(sheet: Sketch, [x, y]: Point, scale = 1): Sketch {
  const hub: Point = [x, y];
  const nodes: Point[] = [
    [x - 16 * scale, y - 10 * scale],
    [x + 16 * scale, y - 10 * scale],
    [x, y + 15 * scale],
  ];
  for (const node of nodes) sheet.rule(hub, node, 1.1, "graphite");
  for (const node of nodes) sheet.blot(node, 3.4 * scale, "graphite");
  return sheet.blot(hub, 4 * scale, "clay");
}

/** A person as a sign-in screen shows one: a head over shoulders. */
function person(sheet: Sketch, [x, y]: Point, ink: "graphite" | "clay"): Sketch {
  return sheet.blot([x, y], 4.6, ink).stroke(
    [
      [x - 8, y + 18],
      [x - 6, y + 13.5],
      [x, y + 11.5],
      [x + 6, y + 13.5],
      [x + 8, y + 18],
    ],
    6.5,
    ink,
    0.3,
  );
}

/** A tick, as a hand marks something done. */
const tick = (sheet: Sketch, [x, y]: Point) =>
  sheet.stroke(
    [
      [x, y],
      [x + 3.5, y + 4],
      [x + 11, y - 7],
    ],
    2.2,
    "clay",
    0.2,
  );

/** The three steps of each way a device comes in by what this one gives
 *  it: another computer by the address and account, a phone by a code it
 *  scans, a Linux host by the line run in its terminal. */
export function joinSteps(way: "computer" | "phone" | "line"): [string, string, string] {
  if (way === "computer") {
    const fields = computer(new Sketch("join-computer-3")).wash(24, 17, 62, 12, "field", 4).wash(24, 35, 62, 12, "field", 4).scribble([29, 23], 30, "graphite");
    for (let k = 0; k < 5; k += 1) fields.blot([31 + k * 6.5, 41], 1.5, "graphite");
    return [
      network(computer(new Sketch("join-computer-1")), [60, 35]).svg(120, 84, STEP),
      computer(new Sketch("join-computer-2")).wash(34, 27, 52, 17, "clay", 8.5).mark('<path d="M5 10h10M10 5v10"/>', [51, 26.5], 18, "paper").svg(120, 84, STEP),
      tick(fields, [90, 40]).svg(120, 84, STEP),
    ];
  }
  if (way === "phone") {
    const code = phone(new Sketch("join-phone-2"));
    for (const [x, y] of [
      [48, 16],
      [62, 16],
      [48, 30],
    ] as Point[])
      code.wash(x, y, 10, 10, "field", 2).wash(x + 3, y + 3, 4, 4, "graphite", 1);
    code.rule([63, 33], [66, 33], 1.6, "graphite").rule([70, 31], [70, 36], 1.6, "graphite").rule([63, 39], [71, 39], 1.6, "graphite");
    const clock = new Sketch("join-phone-3").ring([60, 40], 23, 1.6, "graphite").stroke(
      [
        [60, 40],
        [60, 26],
      ],
      2,
      "clay",
    );
    clock
      .stroke(
        [
          [60, 40],
          [70, 46],
        ],
        2,
        "clay",
      )
      .blot([60, 40], 2.2, "clay");
    return [network(phone(new Sketch("join-phone-1")), [60, 36], 0.7).svg(120, 84, STEP), code.svg(120, 84, STEP), clock.svg(120, 84, STEP)];
  }
  const prompt = computer(new Sketch("join-line-1")).stroke(
    [
      [30, 28],
      [37, 33],
      [30, 38],
    ],
    2.2,
    "clay",
    0.15,
  );
  prompt.scribble([42, 40], 26, "graphite");
  const hosts = new Sketch("join-line-2")
    .wash(24, 14, 72, 18, "box", 4)
    .wash(24, 38, 72, 18, "box", 4)
    .blot([34, 23], 2, "graphite")
    .blot([34, 47], 2, "graphite")
    .scribble([42, 22], 34, "clay")
    .scribble([42, 46], 26, "clay");
  const joined = new Sketch("join-line-3").wash(12, 24, 44, 34, "box", 4).scribble([19, 34], 26, "graphite").scribble([19, 46], 18, "graphite");
  joined.dashed([60, 41], [74, 40], 1.4, "clay", 4, 4);
  return [prompt.svg(120, 84, STEP), hosts.svg(120, 84, STEP), network(joined, [92, 40], 0.75).svg(120, 84, STEP)];
}

/** The three steps, in order: the system account no one at the machine
 *  signs in to, chosen on its sign-in screen; Fastctx installed and open
 *  there; the code pasted where joining asks for it. */
export function managedSteps(): [string, string, string] {
  const chosen = person(person(computer(new Sketch("managed-1")), [42, 24], "graphite"), [78, 24], "clay").ring([78, 33], 17, 1.4, "clay");
  const pasted = computer(new Sketch("managed-3")).wash(24, 26, 56, 14, "field", 4).scribble([29, 33], 42, "clay", 1.3, 1.4);
  return [chosen.svg(120, 84, STEP), network(computer(new Sketch("managed-2")), [60, 35]).svg(120, 84, STEP), tick(pasted, [86, 33]).svg(120, 84, STEP)];
}

/** The machine itself: its one line to the server, and the lines of
 *  agents elsewhere coming in, dashed. */
export function managedMachine(kind = "managed-art"): string {
  const sheet = computer(new Sketch("managed-machine"), 88, 46, 68, 46)
    .wash(192, 16, 24, 24, "box", 5)
    .blot([204, 28], 3.2, "clay")
    .stroke(
      [
        [158, 56],
        [170, 50],
        [181, 43],
        [190, 36],
      ],
      1.4,
      "graphite",
      0.05,
    )
    .blot([30, 34], 6, "graphite")
    .blot([26, 100], 6, "graphite");
  sheet.dashed([40, 38], [82, 60], 1.5, "clay");
  sheet.stroke(
    [
      [76, 52],
      [84, 61],
      [73, 63],
    ],
    1.6,
    "clay",
    0.2,
  );
  sheet.dashed([36, 97], [82, 80], 1.5, "clay");
  sheet.stroke(
    [
      [74, 76],
      [84, 79],
      [76, 86],
    ],
    1.6,
    "clay",
    0.2,
  );
  return sheet.svg(240, 132, kind);
}
