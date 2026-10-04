export type Point = readonly [x: number, y: number];

export type Segment =
  | { readonly kind: 'line'; readonly to: Point }
  | { readonly kind: 'quadratic'; readonly control: Point; readonly to: Point }
  | { readonly kind: 'cubic'; readonly control1: Point; readonly control2: Point; readonly to: Point };

/** A single continuous contour. Coordinates and stroke lengths use view-box units. */
export interface InkPath {
  readonly start: Point;
  readonly segments: readonly Segment[];
  readonly closed?: boolean;
}

export interface InkStyle {
  /** Any CSS color, including var(--accent). Resolved in the canvas's cascade. */
  color: string;
  /** Individual filament width, not the width of the entire bundle. */
  width: number;
  fibers: number;
  /** Full width of the distribution of filament centers. */
  spread: number;
  /** Positive distribution exponent; >1 concentrates filaments near the center. */
  density: number;
  /** Alpha per filament; overlapping filaments accumulate naturally. */
  opacity: number;
  roughness: number;
  jitter: number;
  /** Temporal frequency in cycles per second. */
  jitterSpeed: number;
  /** null means solid. Positive speed moves from path start toward its end. */
  dash: { length: number; gap: number; speed: number; offset?: number } | null;
}

export interface InkStroke {
  path: InkPath;
  style?: Partial<InkStyle>;
  /** Defaults to the stroke ID. Replacing a path keeps the same random identity. */
  seed?: string | number;
}

export interface InkOptions {
  seed?: string | number;
  /** Omit to draw directly in CSS pixels. Fixed boxes use centered 'contain'. */
  viewBox?: readonly [width: number, height: number];
  /** Maximum chord length and curve flatness error, in view-box units. */
  quality?: { step?: number; tolerance?: number };
  autoStart?: boolean;
}

export const defaultStyle: Readonly<InkStyle> = Object.freeze({
  color: 'var(--text)', width: 0.55, fibers: 18, spread: 2.5,
  density: 1.4, opacity: 0.13, roughness: 0.35,
  jitter: 0.22, jitterSpeed: 0.09, dash: null,
});
