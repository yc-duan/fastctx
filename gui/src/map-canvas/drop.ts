import type { CanvasHandle, CanvasItem, Point } from "./types";

export type ValidTarget = (handle: CanvasHandle, item: CanvasItem) => boolean;

/** Resolve overlapping hit areas by distance to the drawn centre, not DOM order. */
export function nearestItem(items: readonly CanvasItem[], screen: Point, project: (point: Point) => Point, scale: number): CanvasItem | null {
  let nearest: CanvasItem | null = null;
  let distance = Infinity;
  let reached: CanvasItem | null = null;
  let off = Infinity;
  for (const item of items) {
    // A rectangle is hit where it is drawn, the first listed that holds the
    // press; failing that, within a finger's height about its middle, the
    // nearest middle where two reach that far, as two strips drawn small
    // close together do. Either stands before any circle.
    if (item.box) {
      const [left, top] = project([item.box.x, item.box.y]);
      const [right, bottom] = project([item.box.x + item.box.width, item.box.y + item.box.height]);
      if (screen[0] < left || screen[0] > right) continue;
      if (screen[1] >= top && screen[1] <= bottom) return item;
      const away = Math.abs(screen[1] - (top + bottom) / 2);
      if (away <= Math.max(22, (bottom - top) / 2) && away < off) { reached = item; off = away; }
      continue;
    }
    const p = project(item.point);
    const d = Math.hypot(screen[0] - p[0], screen[1] - p[1]);
    if (d <= Math.max(22, item.radius * scale) && d < distance) { nearest = item; distance = d; }
  }
  return reached ?? nearest;
}

/** Validation belongs to the page. This same resolver serves pointer and keyboard
 * drops, including a final revalidation at release when network facts have changed. */
export class DropTargets {
  constructor(private readonly valid: ValidTarget) {}
  list(handle: CanvasHandle, items: readonly CanvasItem[]): CanvasItem[] { return items.filter(item => this.valid(handle, item)); }
  at(handle: CanvasHandle, items: readonly CanvasItem[], screen: Point, project: (point: Point) => Point, scale: number): CanvasItem | null {
    // Hit the actual nearest object first. An invalid object cannot expose a valid
    // one behind it when hit areas overlap at distant zoom.
    const nearest = nearestItem(items, screen, project, scale);
    return nearest && this.valid(handle, nearest) ? nearest : null;
  }
}
