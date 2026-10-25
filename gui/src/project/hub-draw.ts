// The project page's hand (`world-gui.md`, 项目主页): a conversation's way
// as its plan walks it, and the ground its machines make. The way is the
// plan's own line laid on its side: the stops walked in ink, the one the
// work stands at a drop of ink, the stops ahead faint; a way not yet sure
// is dashed. Clay only where the user is waited for: a circle round the
// stop the work stands at. Seeded, so a way is drawn the same each time.

import { Sketch } from "../pen/sketch";
import type { Point } from "../pen/shapes";
import type { Station } from "./hub-model";

/** Every way is laid on a track this long, its stops spread along it but
 *  never further apart than a step, so the ways of a project read as
 *  lines side by side; and the room around a way. */
const TRACK = 520;
const STEP = 120;
const EDGE = 12;
const HEIGHT = 28;
/** The most stops drawn; a longer way shows the stretch around where the
 *  work stands, its cut ends trailing off. */
const SHOWN = 9;

/** The stretch of a way that is drawn, and whether stops were left out
 *  before or after it. */
function stretch(route: Station[]): { stations: Station[]; before: boolean; after: boolean } {
  if (route.length <= SHOWN) return { stations: route, before: false, after: false };
  const here = Math.max(0, route.findIndex((one) => one.where === "here"));
  const start = Math.min(Math.max(0, here - 3), route.length - SHOWN);
  return { stations: route.slice(start, start + SHOWN), before: start > 0, after: start + SHOWN < route.length };
}

/** The way, as an inline drawing; `asks` circles where the work stands. */
export function wayDrawing(route: Station[], seed: string, asks: boolean): string {
  const { stations, before, after } = stretch(route);
  const ends = (before ? 1 : 0) + (after ? 1 : 0);
  const step = Math.min(STEP, (TRACK - EDGE * 2) / Math.max(1, stations.length - 1 + ends / 2));
  const lead = before ? step / 2 : 0;
  const width = EDGE * 2 + lead + step * (stations.length - 1) + (after ? step / 2 : 0);
  const y = HEIGHT / 2;
  const at = (index: number): Point => [EDGE + lead + step * index, y];
  const sketch = new Sketch(`hub-way-${seed}`);
  if (before) sketch.dashed([EDGE - 2, y], [at(0)[0] - 6, y], 1.1, "current", 2.5, 3);
  stations.forEach((station, index) => {
    if (index === 0) return;
    const from: Point = [at(index - 1)[0] + 7, y];
    const to: Point = [at(index)[0] - 7, y];
    if (station.where === "ahead" || station.maybe) sketch.dashed(from, to, 1.2, station.where === "ahead" ? "current" : "graphite", 4, 4);
    else sketch.rule(from, to, 1.5, "graphite");
  });
  if (after) sketch.dashed([at(stations.length - 1)[0] + 6, y], [width - EDGE + 2, y], 1.1, "current", 2.5, 3);
  stations.forEach((station, index) => {
    const point = at(index);
    if (station.where === "walked") sketch.blot(point, 3.8, "graphite");
    else if (station.where === "here") {
      sketch.blot(point, 5.6, "graphite");
      if (asks) sketch.ring(point, 10.5, 1.5, "clay");
    } else sketch.ring(point, 4, 1.2, "current");
  });
  return sketch.svg(width, HEIGHT, "ph-way-ink").replace("<svg ", `<svg width="${width}" height="${HEIGHT}" `);
}

/** The ground the machines stand on: one line ruled across, stretched to
 *  the width it is given. */
export function groundDrawing(seed: string): string {
  const sketch = new Sketch(`hub-ground-${seed}`);
  sketch.rule([6, 6], [994, 6.4], 1.4, "graphite");
  return sketch.svg(1000, 12, "ph-ground-ink").replace("<svg ", `<svg preserveAspectRatio="none" `);
}
