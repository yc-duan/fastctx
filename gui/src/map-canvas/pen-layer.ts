import { penDefs } from "../pen/filters";
import { PenTextures } from "./pen-texture";
import type { Bounds, CanvasFrame } from "./types";

export interface PenStroke {
  d: string;
  bounds: Bounds;
  pen: "marker" | "dry" | "rule";
  /** A resolved CSS colour; isolated texture images cannot inherit CSS variables. */
  colour: string;
  opacity?: number;
}
interface Tile { x: number; y: number; width: number; height: number; strokes: PenStroke[]; svg: SVGSVGElement | null; html: string }
let serial = 0;

/** Live vector outlines filled with reusable filtered pen textures. The reference
 * mode keeps whole-stroke filters in visible tiles for direct cost comparisons.
 * Neither mode rebuilds paths when the camera moves. */
export class PenLayer {
  private readonly id = `canvas-pen-${serial++}`;
  private tiles = new Map<string, Tile>();
  private lastScale = NaN;
  private readonly size = 256;
  // Marker displacement reaches 1.3 world units. Eight units also cover the
  // filled stroke's edge and antialiasing, with the tile clipped after filtering.
  private readonly bleed = 8;
  private textures = new PenTextures();
  private generation = 0;
  private frame: CanvasFrame | null = null;
  constructor(private readonly plane: HTMLElement, readonly mode: "texture" | "live-filter" = "texture") {}
  get tileCount(): number { return this.tiles.size; }
  get visibleTileCount(): number { return [...this.tiles.values()].filter(t => t.svg).length; }
  dispose(): void {
    this.generation++;
    for (const tile of this.tiles.values()) tile.svg?.remove();
    this.tiles.clear();
    this.textures.clear();
    this.frame = null;
    this.plane.style.willChange = "auto";
  }
  async update(strokes: readonly PenStroke[]): Promise<void> {
    const generation = ++this.generation;
    const samples = new Map<string, string>();
    if (this.mode === "texture") {
      await Promise.all(strokes.map(async s => { const key = `${s.pen}:${s.colour}`; samples.set(key, await this.textures.get(s.pen, s.colour)); }));
    }
    if (generation !== this.generation) return;
    const next = new Map<string, Tile>();
    if (this.mode === "texture" && strokes.length) {
      const x = Math.min(...strokes.map(s => s.bounds.x)) - this.bleed;
      const y = Math.min(...strokes.map(s => s.bounds.y)) - this.bleed;
      const right = Math.max(...strokes.map(s => s.bounds.x + s.bounds.width)) + this.bleed;
      const bottom = Math.max(...strokes.map(s => s.bounds.y + s.bounds.height)) + this.bleed;
      next.set("drawing", { x, y, width: right - x, height: bottom - y, strokes: [...strokes], svg: this.tiles.get("drawing")?.svg ?? null, html: "" });
    }
    for (const stroke of this.mode === "texture" ? [] : strokes) {
      const b = stroke.bounds;
      for (let y = Math.floor((b.y - this.bleed) / this.size); y <= Math.floor((b.y + b.height + this.bleed) / this.size); y++) {
        for (let x = Math.floor((b.x - this.bleed) / this.size); x <= Math.floor((b.x + b.width + this.bleed) / this.size); x++) {
          const key = `${x},${y}`;
          let tile = next.get(key);
          if (!tile) { tile = { x: x * this.size, y: y * this.size, width: this.size, height: this.size, strokes: [], svg: this.tiles.get(key)?.svg ?? null, html: "" }; next.set(key, tile); }
          tile.strokes.push(stroke);
        }
      }
    }
    for (const [key, tile] of this.tiles) if (!next.has(key)) tile.svg?.remove();
    for (const [key, tile] of next) {
      tile.html = this.compose(tile, key, samples);
      if (tile.svg) {
        if (tile.html !== this.tiles.get(key)?.html) tile.svg.innerHTML = tile.html;
        this.sizeSvg(tile);
      }
    }
    this.tiles = next;
    if (this.frame) this.view(this.frame);
  }
  view(frame: CanvasFrame): void {
    this.frame = frame;
    const v = frame.view;
    const left = -v.x / v.scale, top = -v.y / v.scale;
    const right = left + v.width / v.scale, bottom = top + v.height / v.scale;
    for (const tile of this.tiles.values()) {
      const visible = tile.x < right && tile.x + tile.width > left && tile.y < bottom && tile.y + tile.height > top;
      if (visible && !tile.svg) {
        tile.svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
        this.sizeSvg(tile);
        tile.svg.innerHTML = tile.html;
        this.plane.append(tile.svg);
      } else if (!visible && tile.svg) { tile.svg.remove(); tile.svg = null; }
      if (this.mode === "texture" && tile.svg) this.sizeSvg(tile);
    }
    // Cache translation only. During scale changes allow native SVG re-rastering
    // every frame, so zoom never stretches a frozen filtered bitmap.
    this.plane.style.willChange = this.mode === "live-filter" && frame.moving && v.scale === this.lastScale ? "transform" : "auto";
    this.lastScale = v.scale;
  }
  private sizeSvg(tile: Tile): void {
    if (this.mode === "texture" && this.frame) {
      const v = this.frame.view;
      // Cancel the world's CSS transform around a screen-sized SVG. Its viewBox
      // does the vector projection, limiting raster work to the viewport even
      // when the world itself is large or the camera is very close.
      tile.svg!.setAttribute("viewBox", `${-v.x / v.scale} ${-v.y / v.scale} ${v.width / v.scale} ${v.height / v.scale}`);
      tile.svg!.setAttribute("width", String(v.width));
      tile.svg!.setAttribute("height", String(v.height));
      tile.svg!.style.cssText = `position:absolute;left:0;top:0;overflow:hidden;transform-origin:0 0;transform:matrix(${1 / v.scale},0,0,${1 / v.scale},${-v.x / v.scale},${-v.y / v.scale})`;
      return;
    }
    tile.svg!.setAttribute("viewBox", `${tile.x} ${tile.y} ${tile.width} ${tile.height}`);
    tile.svg!.setAttribute("width", String(tile.width));
    tile.svg!.setAttribute("height", String(tile.height));
    tile.svg!.style.cssText = `position:absolute;left:${tile.x}px;top:${tile.y}px;overflow:hidden`;
  }
  private compose(tile: Tile, key: string, samples: ReadonlyMap<string, string>): string {
    const prefix = `${this.id}-${key.replace(",", "_")}`;
    if (this.mode === "texture") {
      const patterns = [...samples.keys()].map((sample, i) => {
        const size = this.textures.size;
        return `<pattern id="${prefix}-sample-${i}" patternUnits="userSpaceOnUse" width="${size}" height="${size}"><image width="${size}" height="${size}" href="${samples.get(sample)}"/></pattern>`;
      }).join("");
      const keys = [...samples.keys()];
      const groups = new Map<string, { sample: number; opacity: number; paths: string[] }>();
      for (const stroke of tile.strokes) {
        const sample = keys.indexOf(`${stroke.pen}:${stroke.colour}`), opacity = stroke.opacity ?? 1;
        const key = `${sample}:${opacity}`;
        const group = groups.get(key) ?? { sample, opacity, paths: [] };
        group.paths.push(stroke.d);
        groups.set(key, group);
      }
      return `<defs>${patterns}</defs>` + [...groups.values()].map(g => `<path d="${g.paths.join(" ")}" opacity="${g.opacity}" fill="url(#${prefix}-sample-${g.sample})"/>`).join("");
    }
    const defs = penDefs().replaceAll('id="pen-', `id="${prefix}-pen-`).replaceAll(/<filter ([^>]+)>/g, (_match, attributes: string) =>
      `<filter ${attributes.replace(/ x="[^"]*" y="[^"]*" width="[^"]*" height="[^"]*"/, "")} filterUnits="userSpaceOnUse" x="${tile.x - this.bleed}" y="${tile.y - this.bleed}" width="${this.size + this.bleed * 2}" height="${this.size + this.bleed * 2}">`);
    return `<defs>${defs}</defs>` + ["rule", "marker", "dry"].filter(pen => tile.strokes.some(s => s.pen === pen)).map(pen => `<g filter="url(#${prefix}-pen-${pen})">${tile.strokes.filter(s => s.pen === pen).map(s =>
      `<path d="${s.d}" fill="${s.colour}" opacity="${s.opacity ?? 1}"/>`).join("")}</g>`).join("");
  }
}
