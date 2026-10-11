// A feature told in a few hand-drawn frames (`world-gui.md`, 功能): each
// frame drawn stroke by stroke, the next begun when the last is done, a
// short line under each. Where the system asks for less motion the frames
// stand side by side, already drawn.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import { blot, circle, outline, quadratic, ruled, seeded, ticks, type Point } from "../pen/shapes";

const W = 220;
const H = 140;

/** One stroke of a frame, in the colour of one of the pens. */
interface Stroke {
  d: string;
  ink: "graphite" | "ink" | "clay";
}

/** A mark of an agent or a device, inside a drawing. */
interface Mark {
  name: "claude" | "codex" | "desktop" | "server" | "phone" | "check";
  at: Point;
  size: number;
}

interface Frame {
  strokes: Stroke[];
  marks: Mark[];
}

function arrow(from: Point, control: Point, to: Point, random: () => number, dashed: boolean): Stroke[] {
  const points = quadratic(from, control, to, 3);
  const body = dashed ? ticks(points, 9, 5, 1.4, random).map((d) => ({ d, ink: "graphite" as const })) : [{ d: outline(points, 1.6, random, { pressure: 0.2, taper: 0.05 }), ink: "graphite" as const }];
  const [px, py] = points[points.length - 3]!;
  const angle = Math.atan2(to[1] - py, to[0] - px);
  const wing = (turn: number): Point => [to[0] - Math.cos(angle + turn) * 8, to[1] - Math.sin(angle + turn) * 8];
  return [...body, { d: ruled(wing(0.5), to, 1.5, random), ink: "graphite" }, { d: ruled(wing(-0.5), to, 1.5, random), ink: "graphite" }];
}

function scribble(from: Point, width: number, random: () => number): string {
  const points: Point[] = [];
  for (let x = 0; x <= width; x += 4) points.push([from[0] + x, from[1] + Math.sin(x / 3.2 + random()) * 1.6]);
  return outline(points, 1.1, random, { pressure: 0.25, taper: 0.1 });
}

function bubble(at: Point, width: number, height: number, random: () => number, tail: "left" | "right"): string[] {
  const [x, y] = at;
  const edges = [
    ruled([x, y], [x + width, y], 1.4, random),
    ruled([x + width, y], [x + width, y + height], 1.4, random),
    ruled([x + width, y + height], [x, y + height], 1.4, random),
    ruled([x, y + height], [x, y], 1.4, random),
  ];
  const tx = tail === "left" ? x + 14 : x + width - 14;
  edges.push(ruled([tx, y + height], [tx + (tail === "left" ? -6 : 6), y + height + 9], 1.3, random));
  return edges;
}

/** The four frames of model collaboration. */
function collaboration(): Frame[] {
  const random = seeded("collaboration");
  const left: Point = [56, 72];
  const right: Point = [166, 72];
  // A conversation: the user's words on the right, the agent's on the left.
  const one: Frame = {
    strokes: [
      ...bubble([118, 30], 78, 30, random, "right").map((d) => ({ d, ink: "ink" as const })),
      { d: scribble([128, 41], 54, random), ink: "ink" },
      { d: scribble([128, 50], 36, random), ink: "ink" },
      ...bubble([26, 70], 96, 40, random, "left").map((d) => ({ d, ink: "graphite" as const })),
      { d: scribble([36, 82], 70, random), ink: "graphite" },
      { d: scribble([36, 91], 76, random), ink: "graphite" },
      { d: scribble([36, 100], 40, random), ink: "graphite" },
    ],
    marks: [{ name: "claude", at: [12, 104], size: 18 }],
  };
  // It asks a GPT in: a line from one to the other, a letter on its way.
  const two: Frame = {
    strokes: [
      { d: circle(left, 20, 1.6, random), ink: "ink" },
      { d: circle(right, 20, 1.6, random), ink: "ink" },
      ...arrow([78, 62], [111, 34], [143, 60], random, true),
      { d: blot([111, 43], 3.2, random), ink: "clay" },
    ],
    marks: [
      { name: "claude", at: [left[0] - 10, left[1] - 10], size: 20 },
      { name: "codex", at: [right[0] - 10, right[1] - 10], size: 20 },
      { name: "desktop", at: [left[0] - 9, 104], size: 18 },
      { name: "server", at: [right[0] - 9, 104], size: 18 },
    ],
  };
  // The GPT works on its own machine: strokes around it, a while.
  const around = Array.from({ length: 33 }, (_, i): Point => {
    const angle = (i / 32) * Math.PI * 2;
    return [right[0] - 22 + Math.cos(angle) * 34, right[1] - 6 + Math.sin(angle) * 34];
  });
  const three: Frame = {
    strokes: [
      { d: circle([right[0] - 22, right[1] - 6], 22, 1.7, random), ink: "ink" },
      ...ticks(around, 7, 6, 1.4, random).map((d) => ({ d, ink: "clay" as const })),
      { d: ruled([40, 116], [104, 116], 1.2, random), ink: "graphite" },
      { d: ruled([40, 124], [86, 124], 1.2, random), ink: "graphite" },
    ],
    marks: [
      { name: "codex", at: [right[0] - 33, right[1] - 17], size: 22 },
      { name: "server", at: [18, 108], size: 18 },
    ],
  };
  // It hands back: the line comes home, a tick by the conversation.
  const four: Frame = {
    strokes: [
      { d: circle(left, 20, 1.6, random), ink: "ink" },
      { d: circle(right, 20, 1.6, random), ink: "ink" },
      ...arrow([143, 84], [111, 112], [78, 86], random, false),
      { d: outline([[44, 34], [52, 42], [70, 22]], 2.4, random, { pressure: 0.3, taper: 0.2 }), ink: "clay" },
    ],
    marks: [
      { name: "claude", at: [left[0] - 10, left[1] - 10], size: 20 },
      { name: "codex", at: [right[0] - 10, right[1] - 10], size: 20 },
    ],
  };
  return [one, two, three, four];
}

/** A phone, upright, its screen's top edge at `y`. */
function phone(at: Point, random: () => number): string[] {
  const [x, y] = at;
  return [
    ruled([x, y], [x + 46, y], 1.5, random),
    ruled([x + 46, y], [x + 46, y + 82], 1.5, random),
    ruled([x + 46, y + 82], [x, y + 82], 1.5, random),
    ruled([x, y + 82], [x, y], 1.5, random),
    ruled([x + 18, y + 75], [x + 28, y + 75], 1.2, random),
  ];
}

/** A computer's screen on its stand. */
function computer(at: Point, random: () => number): string[] {
  const [x, y] = at;
  return [
    ruled([x, y], [x + 70, y], 1.5, random),
    ruled([x + 70, y], [x + 70, y + 48], 1.5, random),
    ruled([x + 70, y + 48], [x, y + 48], 1.5, random),
    ruled([x, y + 48], [x, y], 1.5, random),
    ruled([x + 35, y + 48], [x + 35, y + 60], 1.4, random),
    ruled([x + 22, y + 61], [x + 48, y + 61], 1.4, random),
  ];
}

/** The four frames of 「对话」: the phone in the hand, the agent at home. */
function conversations(): Frame[] {
  const random = seeded("conversations");
  const ink = (d: string) => ({ d, ink: "ink" as const });
  const graphite = (d: string) => ({ d, ink: "graphite" as const });
  // The phone in your hand, the agent on the computer at home.
  const one: Frame = {
    strokes: [...phone([24, 30], random).map(ink), ...computer([132, 36], random).map(graphite), ...ticks(quadratic([76, 70], [104, 50], [128, 62], 3), 7, 5, 1.3, random).map(graphite)],
    marks: [{ name: "claude", at: [158, 51], size: 18 }],
  };
  // You say a line; it writes back word by word.
  const two: Frame = {
    strokes: [
      ...phone([24, 30], random).map(ink),
      ...bubble([34, 40], 30, 14, random, "right").map(ink),
      { d: scribble([38, 47], 20, random), ink: "ink" },
      ...computer([132, 36], random).map(graphite),
      { d: scribble([140, 48], 52, random), ink: "graphite" },
      { d: scribble([140, 57], 40, random), ink: "graphite" },
      { d: scribble([140, 66], 22, random), ink: "clay" },
    ],
    marks: [],
  };
  // It needs your word: it finds your phone.
  const three: Frame = {
    strokes: [
      ...phone([24, 30], random).map(ink),
      ...computer([132, 36], random).map(graphite),
      ...arrow([128, 54], [100, 28], [74, 50], random, false),
      { d: blot([47, 58], 6, random), ink: "clay" },
    ],
    marks: [{ name: "claude", at: [158, 51], size: 18 }],
  };
  // You were away; one look shows what happened.
  const four: Frame = {
    strokes: [
      ...phone([86, 26], random).map(ink),
      { d: scribble([94, 40], 28, random), ink: "graphite" },
      { d: scribble([94, 52], 22, random), ink: "graphite" },
      { d: scribble([94, 64], 26, random), ink: "graphite" },
      { d: scribble([94, 76], 18, random), ink: "graphite" },
      { d: outline([[126, 38], [130, 42], [137, 33]], 2, random, { pressure: 0.3, taper: 0.2 }), ink: "clay" },
      { d: outline([[126, 62], [130, 66], [137, 57]], 2, random, { pressure: 0.3, taper: 0.2 }), ink: "clay" },
    ],
    marks: [],
  };
  return [one, two, three, four];
}

const STROKE_MS = 90;
const FRAME_GAP_MS = 380;

/** The frames of a feature, drawn in order. */
export function storyboard(feature: "collaboration" | "conversations", captions: readonly string[], label: string): string {
  const frames = feature === "conversations" ? conversations() : collaboration();
  let start = 0;
  const html = frames.map((frame, index) => {
    const strokes = frame.strokes
      .map((stroke, i) => `<path class="sb-stroke sb-${stroke.ink}" d="${stroke.d}" style="animation-delay:${start + i * STROKE_MS}ms"/>`)
      .join("");
    const marks = frame.marks
      .map((mark, i) => `<g class="sb-mark" transform="translate(${mark.at[0]} ${mark.at[1]})" style="animation-delay:${start + (frame.strokes.length + i) * STROKE_MS}ms">${glyph(mark.name, mark.size)}</g>`)
      .join("");
    const done = start + (frame.strokes.length + frame.marks.length) * STROKE_MS;
    const caption = `<figcaption style="animation-delay:${done}ms">${escape(captions[index] ?? "")}</figcaption>`;
    start = done + FRAME_GAP_MS;
    return `<figure class="sb-frame"><svg viewBox="0 0 ${W} ${H}" aria-hidden="true"><g filter="url(#pen-marker-lite)">${strokes}</g>${marks}</svg>${caption}</figure>`;
  });
  return `<div class="storyboard" role="img" aria-label="${escape(`${label}: ${captions.join("; ")}`)}">${html.join("")}</div>`;
}
