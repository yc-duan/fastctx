import type { Bounds, Point } from "./types";

export interface ScreenLabel { key: string; point: Point; radius: number; width: number; height: number; priority: number }
export interface LabelPlacement { key: string; box: Bounds | null }
export function intersects(a: Bounds, b: Bounds): boolean {
  return a.x < b.x + b.width && a.x + a.width > b.x && a.y < b.y + b.height && a.y + a.height > b.y;
}

/** Screen-space names keep their measured font size. A crowded name is withheld,
 * never shrunk or clipped; the caller receives every withheld key. */
export function placeLabels(labels: readonly ScreenLabel[], viewport: Bounds, obstacles: readonly Bounds[] = [], gap = 4): LabelPlacement[] {
  const occupied = [...obstacles];
  const placements: LabelPlacement[] = [];
  for (const label of [...labels].sort((a, b) => b.priority - a.priority || a.key.localeCompare(b.key))) {
    const { key, point: [x, y], radius, width, height } = label;
    const candidates = [
      { x: x - width / 2, y: y + radius + gap, width, height },
      { x: x - width / 2, y: y - radius - gap - height, width, height },
      { x: x + radius + gap, y: y - height / 2, width, height },
      { x: x - radius - gap - width, y: y - height / 2, width, height },
    ];
    const box = candidates.find(b => b.x >= viewport.x && b.y >= viewport.y && b.x + width <= viewport.x + viewport.width &&
      b.y + height <= viewport.y + viewport.height && !occupied.some(o => intersects(b, o))) ?? null;
    placements.push({ key, box });
    if (box) occupied.push({ x: box.x - gap / 2, y: box.y - gap / 2, width: width + gap, height: height + gap });
  }
  return placements;
}
