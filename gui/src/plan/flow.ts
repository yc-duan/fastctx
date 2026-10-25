// A flow written as an itinerary, the one way the plan draws a flow: a step
// a line, the stops strung on an ink line down the margin. Where the way
// forks, the steps that may follow are set in under the fork, each on a
// short way of its own from the line, and the line goes on to where they
// meet again. The steps walked are in the quieter ink, the step the work
// stands at is circled in clay with dots beneath it, the steps ahead are
// small rings, and a way not yet sure is dashed. What a way says is noted
// after the step it leads to.

import { Sketch } from "../pen/sketch";
import type { Point } from "../pen/shapes";
import { escape } from "../conversation/prose";
import type { Flow } from "./source";

export interface Stop {
  label: string;
  note: string;
  /** Set in under a fork, one of the steps that may follow it. */
  branch: boolean;
  /** Every way into it is dashed. */
  maybe: boolean;
  where: "" | "walked" | "here" | "ahead";
}

/** The flow's steps in the order they are walked: by the longest way in
 *  from the start, a loop cut where it closes. */
export function stops(flow: Flow): Stop[] {
  const index = new Map(flow.nodes.map((node, at) => [node.key, at]));
  const rank = flow.nodes.map(() => 0);
  for (let round = 0; round < flow.nodes.length; round += 1) {
    let moved = false;
    for (const edge of flow.edges) {
      const from = index.get(edge.from)!;
      const to = index.get(edge.to)!;
      if (rank[to]! < rank[from]! + 1 && rank[from]! + 1 < flow.nodes.length) {
        rank[to] = rank[from]! + 1;
        moved = true;
      }
    }
    if (!moved) break;
  }
  const layers: number[][] = [];
  flow.nodes.forEach((_, at) => (layers[rank[at]!] ??= []).push(at));
  const placed = layers.filter(Boolean);
  const strongLayer = placed.findIndex((layer) => layer.some((at) => flow.nodes[at]!.strong));
  const result: Stop[] = [];
  placed.forEach((layer, depth) => {
    for (const at of layer) {
      const node = flow.nodes[at]!;
      const into = flow.edges.filter((edge) => edge.to === node.key);
      const where = strongLayer < 0 ? "" : depth < strongLayer ? "walked" : node.strong ? "here" : "ahead";
      result.push({
        label: node.label,
        note: node.note || (into.find((edge) => edge.label)?.label ?? ""),
        branch: layer.length > 1,
        maybe: into.length > 0 && into.every((edge) => edge.dashed),
        where,
      });
    }
  });
  return result;
}

export function flowElement(flow: Flow, seed: string): HTMLElement {
  const root = document.createElement("div");
  root.className = "pb-route";
  root.dataset.seed = seed;
  root.innerHTML = `<div class="pb-ink" aria-hidden="true"></div>${stops(flow)
    .map(
      (stop) =>
        `<div class="pb-stop${stop.where ? ` ${stop.where}` : ""}${stop.branch ? " branch" : ""}${stop.maybe ? " maybe" : ""}"><span class="pb-stop-dot"></span><span class="pb-stop-body"><span class="pb-stop-name">${escape(stop.label)}</span>${
          stop.note ? `<span class="pb-stop-note">${escape(stop.note)}</span>` : ""
        }</span></div>`,
    )
    .join("")}`;
  return root;
}

/** The margin's ink line through the stops on the way, a short way out to
 *  each step set in under a fork, and each stop's mark. */
export function drawFlow(root: HTMLElement): void {
  const rows = [...root.querySelectorAll<HTMLElement>(".pb-stop")];
  if (!rows.length) return;
  const box = root.getBoundingClientRect();
  const center = (row: HTMLElement): Point => {
    const r = row.querySelector<HTMLElement>(".pb-stop-dot")!.getBoundingClientRect();
    return [r.left - box.left + r.width / 2, r.top - box.top + 12];
  };
  const marks = rows.map((row) => ({ row, at: center(row), branch: row.classList.contains("branch") }));
  const spine = marks.find((mark) => !mark.branch);
  const x = spine ? spine.at[0] : marks[0]!.at[0] - 24;
  const sketch = new Sketch(`flow-${root.dataset.seed ?? ""}`);
  let last: Point | null = null;
  for (const mark of marks) {
    const at: Point = mark.branch ? [x, mark.at[1]] : mark.at;
    const maybe = mark.row.classList.contains("maybe");
    if (last) {
      const from: Point = [x, last[1] + 7];
      const to: Point = [x, at[1] - (mark.branch ? 0 : 7)];
      if (to[1] > from[1]) {
        if (maybe && !mark.branch) sketch.dashed(from, to, 1.2, "graphite", 3.5, 3.5);
        else sketch.rule(from, to, 1.25, "graphite");
      }
    }
    if (mark.branch) {
      const out: Point = [mark.at[0] - 7, mark.at[1]];
      if (maybe) sketch.dashed([x, mark.at[1]], out, 1.2, "graphite", 3, 3);
      else sketch.stroke(curve([x, mark.at[1] - 9], [x, mark.at[1]], out), 1.2, "graphite", 0.1);
    }
    last = at;
  }
  for (const mark of marks) {
    const row = mark.row;
    if (row.classList.contains("here")) sketch.blot(mark.at, 3.4, "clay").ring(mark.at, 8.5, 1.4, "clay");
    else if (row.classList.contains("walked")) sketch.blot(mark.at, 3.1, "graphite");
    else sketch.ring(mark.at, 3.6, 1.15, "graphite");
  }
  root.querySelector<HTMLElement>(".pb-ink")!.innerHTML = sketch.svg(root.clientWidth, root.clientHeight, "pb-ink-svg");
}

function curve(from: Point, control: Point, to: Point): Point[] {
  const steps = 10;
  return Array.from({ length: steps + 1 }, (_, at) => {
    const t = at / steps;
    const u = 1 - t;
    return [u * u * from[0] + 2 * u * t * control[0] + t * t * to[0], u * u * from[1] + 2 * u * t * control[1] + t * t * to[1]] as Point;
  });
}
