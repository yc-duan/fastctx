// The agents' reach, drawn above their cards (`agents-overview.ts`): the
// machine shown, as the kind of computer it is, with the marks of the
// agents found on it on its screen, connected ones in ink; and a stroke of
// ink from it to each machine of the network those agents reach through
// `run`. A machine away is drawn faint and not reached; with no agent
// connected nothing is reached yet. Drawn only where there is another
// machine to reach: on a computer of its own, the cards say it all.

import { agentGlyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import { Sketch } from "../pen/sketch";
import type { Pickable } from "./agent-machines";

export interface Reach {
  machine: Pickable;
  /** Each agent found there, and whether a login of it is connected. */
  agents: { agent: string; connected: boolean }[];
  /** The other machines its agents may reach, online first. */
  others: Pickable[];
}

/** The drawing's height; its width is the column's, wide or narrow. */
const HEIGHT = 164;
/** The most machines drawn to the right; past that, a line says how many
 *  more. */
const MOST = 5;

/** The drawing for a column `width` wide; `more` says how many machines
 *  past those drawn. */
export function reachDrawing(reach: Reach, width: number, more: (n: number) => string): string {
  const sketch = new Sketch(`reach-${reach.machine.name}-${width}`);
  const narrow = width < 500;
  const screen = { x: narrow ? 14 : 36, y: 30, w: narrow ? 116 : 150, h: narrow ? 76 : 92 };
  const extra: string[] = [];
  drawMachine(sketch, reach.machine.form, screen);
  // The agents' marks on its screen, in a row.
  const size = narrow ? 18 : 22;
  const gap = narrow ? 8 : 10;
  const row = reach.agents.length * size + (reach.agents.length - 1) * gap;
  reach.agents.forEach(({ agent, connected }, index) => {
    const x = screen.x + (screen.w - row) / 2 + index * (size + gap);
    const y = screen.y + (screen.h - size) / 2;
    extra.push(`<g class="ao-reach-mark${connected ? "" : " faint"}" transform="translate(${x} ${y})">${agentGlyph(agent, size)}</g>`);
  });
  extra.push(`<text class="ao-reach-name here" x="${screen.x + screen.w / 2}" y="${HEIGHT - 6}" text-anchor="middle">${escape(reach.machine.written)}</text>`);

  const reaching = reach.agents.some((one) => one.connected);
  const shown = reach.others.length > MOST ? reach.others.slice(0, MOST - 1) : reach.others;
  const rows = shown.length + (reach.others.length > shown.length ? 1 : 0);
  const step = Math.min(32, (HEIGHT - 24) / Math.max(rows, 1));
  const top = HEIGHT / 2 - ((rows - 1) * step) / 2 - 8;
  const left = width - (narrow ? 138 : 236);
  const from: [number, number] = [screen.x + screen.w + 6, screen.y + screen.h / 2];
  shown.forEach((other, index) => {
    const y = top + index * step;
    const away = other.online === false;
    if (reaching && !away) {
      // Level as it leaves and as it arrives, turning between: a hand's
      // line, not a fan of rulings.
      const span = left - 10 - from[0];
      const curve: [number, number][] = [];
      for (let step = 0; step <= 8; step += 1) {
        const t = step / 8;
        const ease = t * t * (3 - 2 * t);
        curve.push([from[0] + span * t, from[1] + (y - from[1]) * ease]);
      }
      sketch.stroke(curve, 1.5, "graphite");
    }
    sketch.wash(left, y - 11, 32, 21, away ? "field" : "box", 4);
    sketch.rule([left + 10, y + 13], [left + 22, y + 13], 1.5, "graphite");
    extra.push(`<text class="ao-reach-name${away ? " away" : ""}" x="${left + 42}" y="${y + 5}">${escape(clip(other.written, narrow ? 11 : 18))}</text>`);
  });
  if (rows > shown.length) {
    extra.push(`<text class="ao-reach-name away" x="${left + 42}" y="${top + shown.length * step + 5}">${escape(more(reach.others.length - shown.length))}</text>`);
  }
  if (reaching && shown.some((other) => other.online !== false)) sketch.blot(from, 3, "clay");
  return sketch.svg(width, HEIGHT, `ao-reach-${narrow ? "narrow" : "wide"}`).replace("</svg>", `${extra.join("")}</svg>`);
}

/** The machine as the kind of computer it is: a screen on a stand, a
 *  laptop's screen over its base, or a server's stacked units. */
function drawMachine(sketch: Sketch, form: Pickable["form"], screen: { x: number; y: number; w: number; h: number }): void {
  const { x, y, w, h } = screen;
  if (form === "server") {
    const unit = (h - 8) / 3;
    for (let index = 0; index < 3; index += 1) {
      sketch.wash(x, y + index * (unit + 4), w, unit, "box", 5);
      sketch.rule([x + 10, y + index * (unit + 4) + unit / 2], [x + 22, y + index * (unit + 4) + unit / 2], 1.6, "graphite");
    }
    return;
  }
  sketch.wash(x, y, w, h, "box", 8);
  if (form === "laptop") {
    sketch.wash(x - 12, y + h + 4, w + 24, 8, "box", 4);
    return;
  }
  sketch.rule([x + w / 2, y + h + 2], [x + w / 2, y + h + 12], 1.8, "graphite");
  sketch.rule([x + w / 2 - 22, y + h + 14], [x + w / 2 + 22, y + h + 14], 1.8, "graphite");
}

function clip(name: string, most: number): string {
  return name.length > most ? `${name.slice(0, most - 1)}…` : name;
}
