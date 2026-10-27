import type { Point } from "../pen/shapes";

export type { Point };
export interface Bounds { x: number; y: number; width: number; height: number }
export interface View extends Bounds { scale: number; level: number }

/** Keys are unique across machines and hubs. Coordinates belong to the layout. */
export interface CanvasItem {
  key: string;
  kind: "hub" | "machine" | "group";
  point: Point;
  radius: number;
  /** Extent of pen marks around the ring, for keeping names clear of details. */
  labelRadius?: number;
  label: string;
  sub?: string;
  priority?: number;
  /** The side its name goes first, when there is room there. */
  side?: "below" | "above" | "right" | "left";
  /** Pressed over this rectangle instead of the circle round `point`, its
   *  corners rounded by `corners` (top left, top right, bottom right, bottom
   *  left), all in the drawing's units; without them, as the page styles
   *  it. */
  box?: { x: number; y: number; width: number; height: number; corners?: [number, number, number, number] };
  /** Found only by where it is, as a press finds every item: no hit area
   *  of its own, so not reached by the keyboard. */
  quiet?: boolean;
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
  /** An item pressed; `add` while Ctrl (Command on a Mac) is held, to add
   *  it to those chosen or take it out. */
  | { kind: "select"; key: string | null; at: Point; add: boolean }
  | { kind: "context"; key: string | null; at: Point; input: "mouse" | "touch" | "keyboard" }
  /** An item pressed twice, to be opened. */
  | { kind: "open"; key: string }
  | { kind: "draft"; draft: Draft }
  | { kind: "drop"; handle: CanvasHandle; target: string; at: Point }
  /** A frame drawn with the left mouse button: the items inside it, and
   *  where it is in the drawing's units, while it is drawn and once it is let
   *  go; `add` while Ctrl or Command is held, to add them to those chosen. */
  | { kind: "marquee"; keys: string[]; area: Bounds; done: boolean; add: boolean }
  | { kind: "cancel"; handle: CanvasHandle; reason: "escape" | "outside" | "interrupted" | "changed" };

export interface CanvasFrame {
  view: View;
  moving: boolean;
  reducedMotion: boolean;
  /** Names hidden by collision or the viewport; all items remain keyboard reachable. */
  hiddenLabels: readonly string[];
}
