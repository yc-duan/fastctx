import type { MapHub, MapNode } from "../app/map-nodes";
import type { Bounds, CanvasItem, Point } from "./types";

export interface LayoutSnapshot { hubs: [string, Point][]; machines: [string, Point][] }
export interface NetworkLayout { items: CanvasItem[]; hubs: ReadonlyMap<string, Point>; machines: ReadonlyMap<string, Point>; bounds: Bounds }

function* spiral(): Generator<Point> {
  yield [0, 0];
  for (let ring = 1; ; ring++) {
    for (let x = -ring; x <= ring; x++) yield [x, -ring];
    for (let y = -ring + 1; y <= ring; y++) yield [ring, y];
    for (let x = ring - 1; x >= -ring; x--) yield [x, ring];
    for (let y = ring - 1; y > -ring; y--) yield [-ring, y];
  }
}

/** Incremental computed layout. Surviving and returning identities never move.
 * Removed identities reserve their cells until this layout is discarded. A snapshot
 * can carry those reservations across page instances; it contains no user positions. */
export class StableNetworkLayout {
  private hubs = new Map<string, Point>();
  private machines = new Map<string, Point>();
  private occupied = new Set<string>();
  readonly pitch = 120;
  constructor(snapshot?: LayoutSnapshot) {
    if (snapshot) {
      for (const [key, point] of snapshot.hubs) this.reserve(this.hubs, key, point);
      for (const [key, point] of snapshot.machines) this.reserve(this.machines, key, point);
    }
  }
  snapshot(): LayoutSnapshot { return { hubs: [...this.hubs], machines: [...this.machines] }; }
  private reserve(map: Map<string, Point>, key: string, point: Point): void {
    if (map.has(key) || !point.every(Number.isFinite) || point.some(n => n % this.pitch !== 0)) throw new Error("Invalid layout reservation");
    const cell = `${point[0]},${point[1]}`;
    if (this.occupied.has(cell)) throw new Error("Duplicate layout cell");
    map.set(key, [point[0], point[1]]);
    this.occupied.add(cell);
  }
  private allocate(near: Point, step: number): Point {
    const cx = Math.round(near[0] / this.pitch) * this.pitch;
    const cy = Math.round(near[1] / this.pitch) * this.pitch;
    for (const [x, y] of spiral()) {
      const point: Point = [cx + x * step, cy + y * step];
      if (!this.occupied.has(`${point[0]},${point[1]}`)) return point;
    }
    throw new Error("Layout exhausted");
  }
  update(hubs: readonly MapHub[], nodes: readonly MapNode[]): NetworkLayout {
    if (new Set(hubs.map(h => h.id)).size !== hubs.length || new Set(nodes.map(n => n.key)).size !== nodes.length || nodes.some(n => hubs.some(h => `hub:${h.id}` === n.key))) {
      throw new Error("Duplicate network identity");
    }
    for (const hub of [...hubs].sort((a, b) => a.id.localeCompare(b.id))) {
      if (!this.hubs.has(hub.id)) this.reserve(this.hubs, hub.id, this.allocate([0, 0], this.pitch * 6));
    }
    const activeHubs = new Map(hubs.map(h => [h.id, this.hubs.get(h.id)!]));
    for (const node of [...nodes].sort((a, b) => a.key.localeCompare(b.key))) {
      if (this.machines.has(node.key)) continue;
      const on = [...new Set(node.hubs)].flatMap(id => activeHubs.has(id) ? [activeHubs.get(id)!] : []);
      const near: Point = on.length ? [on.reduce((s, p) => s + p[0], 0) / on.length, on.reduce((s, p) => s + p[1], 0) / on.length] : [0, 0];
      this.reserve(this.machines, node.key, this.allocate(near, this.pitch));
    }
    const machines = new Map(nodes.map(n => [n.key, this.machines.get(n.key)!]));
    const items: CanvasItem[] = [
      ...hubs.map(h => ({ key: `hub:${h.id}`, kind: "hub" as const, point: activeHubs.get(h.id)!, radius: 10, label: h.name, priority: 1 })),
      ...nodes.map(n => ({ key: n.key, kind: "machine" as const, point: machines.get(n.key)!, radius: 26, label: n.name, sub: n.sub })),
    ];
    const left = Math.min(0, ...items.map(n => n.point[0] - n.radius));
    const top = Math.min(0, ...items.map(n => n.point[1] - n.radius));
    const right = Math.max(0, ...items.map(n => n.point[0] + n.radius));
    const bottom = Math.max(0, ...items.map(n => n.point[1] + n.radius));
    return { items, hubs: activeHubs, machines, bounds: { x: left, y: top, width: right - left, height: bottom - top } };
  }
}
