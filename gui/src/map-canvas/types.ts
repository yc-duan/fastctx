import type { Point } from "../pen/shapes";

export type { Point };
export interface Bounds { x: number; y: number; width: number; height: number }
export interface View extends Bounds { scale: number; level: number }

/** Keys are unique across machines and hubs. Coordinates belong to the layout. */
export interface CanvasItem {
  key: string;
  kind: "hub" | "machine";
  point: Point;
  radius: number;
  /** Extent of pen marks around the ring, for keeping names clear of details. */
  labelRadius?: number;
  label: string;
  sub?: string;
  priority?: number;
}

/** A handle starts an intent; the page validates targets and confirms the result. */
export interface CanvasHandle {
  key: string;
  source: string;
  point: Point;
  label: string;
}

export interface Draft {
  handle: CanvasHandle;
  point: Point;
  target: string | null;
  valid: readonly string[];
  keyboard: boolean;
}

export type CanvasEvent =
  | { kind: "point"; key: string | null; at: Point }
  | { kind: "select"; key: string | null; at: Point }
  | { kind: "context"; key: string | null; at: Point; input: "mouse" | "touch" | "keyboard" }
  | { kind: "draft"; draft: Draft }
  | { kind: "drop"; handle: CanvasHandle; target: string; at: Point }
  | { kind: "cancel"; handle: CanvasHandle; reason: "escape" | "outside" | "interrupted" | "changed" };

export interface CanvasFrame {
  view: View;
  moving: boolean;
  reducedMotion: boolean;
  /** Names hidden by collision or the viewport; all items remain keyboard reachable. */
  hiddenLabels: readonly string[];
}
