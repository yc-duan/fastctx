import type { Bounds, Point, View } from "./types";

export interface CameraOptions {
  minScale: number;
  maxScale: number;
  /** Ascending zoom boundaries. The page supplies the meaning of each level. */
  levels: readonly number[];
}

/** Screen = world * scale + translation. All movement is direct, without inertia. */
export class CanvasCamera {
  private state: View = { x: 0, y: 0, width: 0, height: 0, scale: 1, level: 0 };
  constructor(readonly options: CameraOptions) {
    if (![options.minScale, options.maxScale].every(Number.isFinite) || !(options.minScale > 0 && options.maxScale >= options.minScale) ||
      options.levels.some((n, i, all) => !Number.isFinite(n) || n <= 0 || i > 0 && n <= all[i - 1])) {
      throw new Error("Invalid camera scales");
    }
    this.set({ ...this.state, scale: Math.max(options.minScale, Math.min(options.maxScale, 1)) });
  }
  get view(): View { return { ...this.state }; }
  set(view: View): void {
    if (![view.x, view.y, view.width, view.height, view.scale].every(Number.isFinite) || view.width < 0 || view.height < 0) {
      throw new Error("Invalid camera view");
    }
    const scale = Math.max(this.options.minScale, Math.min(this.options.maxScale, view.scale));
    this.state = { ...view, scale, level: this.options.levels.filter(n => scale >= n).length };
  }
  resize(width: number, height: number): void {
    this.set({ ...this.state, width, height, x: this.state.x + (width - this.state.width) / 2, y: this.state.y + (height - this.state.height) / 2 });
  }
  toScreen(point: Point): Point { return [point[0] * this.state.scale + this.state.x, point[1] * this.state.scale + this.state.y]; }
  toWorld(point: Point): Point { return [(point[0] - this.state.x) / this.state.scale, (point[1] - this.state.y) / this.state.scale]; }
  pan(dx: number, dy: number): void { this.set({ ...this.state, x: this.state.x + dx, y: this.state.y + dy }); }
  zoom(factor: number, at: Point = [this.state.width / 2, this.state.height / 2]): void {
    if (!Number.isFinite(factor) || factor <= 0) throw new Error("Invalid zoom factor");
    const world = this.toWorld(at);
    const scale = Math.max(this.options.minScale, Math.min(this.options.maxScale, this.state.scale * factor));
    this.set({ ...this.state, scale, x: at[0] - world[0] * scale, y: at[1] - world[1] * scale });
  }
  /** Padding is in CSS pixels, so fit leaves room for unscaled names and controls.
   *  No smaller than `floor`: each way a drawing is larger than its view at
   *  that scale, it shows `focus` in the middle, or without one its start,
   *  as a page is read, never past its own edges; each way it fits, it is
   *  centred. */
  fit(bounds: Bounds, padding: number, floor = 0, focus: Bounds | null = null): void {
    const whole = Math.min((this.state.width - padding * 2) / Math.max(1, bounds.width), (this.state.height - padding * 2) / Math.max(1, bounds.height));
    const scale = Math.max(this.options.minScale, Math.min(this.options.maxScale, Math.max(whole, floor)));
    const axis = (start: number, size: number, view: number, near: number | undefined, far: number | undefined): number => {
      if (size * scale <= view - padding * 2) return view / 2 - (start + size / 2) * scale;
      const first = padding - start * scale;
      const last = view - padding - (start + size) * scale;
      const wanted = near === undefined || far === undefined
        ? first
        : far * scale <= view - padding * 2 ? view / 2 - (near + far / 2) * scale : padding - near * scale;
      return Math.min(first, Math.max(last, wanted));
    };
    this.set({ ...this.state, scale,
      x: axis(bounds.x, bounds.width, this.state.width, focus?.x, focus?.width),
      y: axis(bounds.y, bounds.height, this.state.height, focus?.y, focus?.height) });
  }
  reveal(point: Point, margin: number): void {
    const [x, y] = this.toScreen(point);
    const mx = Math.min(margin, this.state.width / 2);
    const my = Math.min(margin, this.state.height / 2);
    this.pan(x < mx ? mx - x : x > this.state.width - mx ? this.state.width - mx - x : 0,
      y < my ? my - y : y > this.state.height - my ? this.state.height - my - y : 0);
  }
}
