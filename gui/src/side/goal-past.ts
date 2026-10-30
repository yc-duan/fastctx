// The goals a conversation had before the one it has now (`world-goal.md`,
// 以前的目标), on its goal's page: each a station on an ink line down the
// margin, the latest first, a tick in clay for one done and a hollow ring
// for one put down. The colleagues it asked while a goal was on hang from
// that goal on a curved branch, each marked as its own goal stands, so the
// page reads as the conversation's goals and the goals they gave rise to.
// A goal with an account of it done opens that below it; a colleague opens
// beside the conversation.

import { escape, render } from "../conversation/prose";
import { glyph } from "../conversation/glyphs";
import { Sketch } from "../pen/sketch";
import { sizedInk } from "../pen/sized-ink";
import type { Point } from "../pen/shapes";
import type { CollabNode, GoalStatus, PastGoal } from "./types";
import type { GoalWords } from "./words";

/** The section's markup, none where the conversation had no goal before. */
export function pastHtml(past: PastGoal[], team: CollabNode[], words: GoalWords, now: number): string {
  if (!past.length) return "";
  const rows = [...past]
    .sort((a, b) => b.closed - a.closed)
    .map((goal) => {
      const when = words.span(words.at(goal.created, now), words.at(goal.closed, now));
      const facts = [goal.standing ? words.standing : "", goal.done ? words.outcome.done : words.outcome.dropped, when];
      const title = escape(`${goal.objective}\n${facts.filter(Boolean).join(words.separator)}`);
      const inner = `<span class="gh-dot" aria-hidden="true"></span><span class="gh-words">${escape(goal.objective)}</span>`;
      const head = goal.statement
        ? `<button class="gh-goal" type="button" aria-expanded="false" title="${title}">${inner}</button><div class="gh-said prose" hidden>${render(goal.statement)}</div>`
        : `<div class="gh-goal" title="${title}">${inner}</div>`;
      const branches = team
        .filter((node) => node.kind === "colleague" && node.branchOf === goal.id)
        .map((node) => {
          const said = escape([node.name, node.goal ? words.states[node.goal] : "", node.task].filter(Boolean).join("\n"));
          return `<button class="gh-branch" type="button" data-node="${escape(node.id)}" data-goal="${node.goal ?? ""}" title="${said}"><span class="gh-dot" aria-hidden="true"></span><span class="cp-seal">${glyph(node.agent, 14)}</span><span class="gh-words">${escape(node.task || node.name)}</span></button>`;
        })
        .join("");
      return `<div class="gh-row" data-done="${goal.done}">${head}</div>${branches}`;
    })
    .join("");
  return `<section class="gp-part gh-past"><h2 class="gp-label">${escape(words.earlier)}</h2><div class="gh-line"><div class="gh-ink" aria-hidden="true"></div>${rows}</div></section>`;
}

/** Lays the ink down for the width the section has now: the line through
 *  the goals, a branch out to each colleague, and each station's mark. */
export function drawPast(root: HTMLElement): void {
  const line = root.querySelector<HTMLElement>(".gh-line");
  const ink = line?.querySelector<HTMLElement>(".gh-ink");
  if (!line || !ink || !line.clientWidth) return;
  const box = line.getBoundingClientRect();
  const center = (dot: HTMLElement): Point => {
    const r = dot.getBoundingClientRect();
    return [r.left - box.left + r.width / 2, r.top - box.top + r.height / 2];
  };
  const stations = [...line.querySelectorAll<HTMLElement>(".gh-goal, .gh-branch")].map((row) => ({
    at: center(row.querySelector<HTMLElement>(".gh-dot")!),
    branch: row.classList.contains("gh-branch"),
    status: (row.dataset.goal || null) as GoalStatus | null,
    done: row.closest<HTMLElement>(".gh-row")?.dataset.done === "true",
  }));
  const spine = stations.find((one) => !one.branch);
  if (!spine) return;
  const shape = JSON.stringify(stations.map(({ at, branch, status, done }) => [at, branch, status, done]));
  sizedInk(ink, shape, line.clientWidth, line.clientHeight, () => {
    const x = spine.at[0];
    const sketch = new Sketch("goal-past");
    let last: Point | null = null;
    for (const station of stations) {
      const at: Point = station.branch ? [x, station.at[1]] : station.at;
      if (last) {
        const from: Point = [x, last[1] + 7];
        const to: Point = [x, at[1] - (station.branch ? 0 : 7)];
        if (to[1] > from[1]) sketch.rule(from, to, 1.25, "graphite");
      }
      if (station.branch) sketch.stroke(curve([x, station.at[1] - 9], [x, station.at[1]], [station.at[0] - 7, station.at[1]]), 1.2, "graphite", 0.1);
      else last = at;
    }
    for (const station of stations) {
      if (station.branch) mark(sketch, station.at, station.status);
      else if (station.done) tick(sketch, station.at);
      else sketch.ring(station.at, 3.6, 1.15, "graphite");
    }
    return sketch.svg(line.clientWidth, line.clientHeight, "gh-ink-svg");
  });
}

/** A colleague's station, as its own goal stands. */
function mark(sketch: Sketch, at: Point, status: GoalStatus | null): void {
  const [x, y] = at;
  if (status === "complete") tick(sketch, at, 0.8);
  else if (status === "active") sketch.blot(at, 2.8, "graphite");
  else if (status === "paused") sketch.rule([x - 2.2, y - 3.4], [x - 2.2, y + 3.4], 1.3, "graphite").rule([x + 2.2, y - 3.4], [x + 2.2, y + 3.4], 1.3, "graphite");
  else if (status === "blocked") sketch.rule([x - 4.4, y + 0.3], [x + 4.4, y - 0.3], 2, "clay");
  else sketch.ring(at, 3, 1.1, "graphite");
}

/** A tick in clay, centred on a station. */
function tick(sketch: Sketch, [x, y]: Point, scale = 1): void {
  sketch.stroke(
    [
      [x - 5 * scale, y],
      [x - 1.6 * scale, y + 3.6 * scale],
      [x + 5.4 * scale, y - 4.6 * scale],
    ],
    1.7 * scale,
    "clay",
  );
}

function curve(from: Point, control: Point, to: Point): Point[] {
  const steps = 10;
  return Array.from({ length: steps + 1 }, (_, at) => {
    const t = at / steps;
    const u = 1 - t;
    return [u * u * from[0] + 2 * u * t * control[0] + t * t * to[0], u * u * from[1] + 2 * u * t * control[1] + t * t * to[1]] as Point;
  });
}
