import { penDefs } from "../pen/filters";

/** A reusable filtered grain sample. Filled stroke silhouettes stay vector sharp;
 * noise and its displacement are computed once per pen and colour, not per view.
 * The seeded outlines supply the edge's waver rather than filtering an entire map. */
export class PenTextures {
  private cache = new Map<string, Promise<string>>();
  readonly size = 128;
  get(pen: "marker" | "dry" | "rule", colour: string): Promise<string> {
    const key = `${pen}:${colour}`;
    let sample = this.cache.get(key);
    if (!sample) { sample = this.make(pen, colour); this.cache.set(key, sample); }
    return sample;
  }
  clear(): void { this.cache.clear(); }
  private async make(pen: string, colour: string): Promise<string> {
    const size = this.size;
    const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="${size}" height="${size}" viewBox="0 0 ${size} ${size}"><defs>${penDefs()}</defs><g filter="url(#pen-${pen})"><rect x="-8" y="-8" width="${size + 16}" height="${size + 16}" fill="${colour}"/></g></svg>`;
    const url = URL.createObjectURL(new Blob([svg], { type: "image/svg+xml" }));
    const image = new Image();
    image.src = url;
    try {
      await image.decode();
      const canvas = document.createElement("canvas");
      canvas.width = canvas.height = size;
      const context = canvas.getContext("2d");
      if (!context) throw new Error("Pen texture requires a 2D canvas");
      context.drawImage(image, 0, 0);
      return canvas.toDataURL("image/png");
    } finally { URL.revokeObjectURL(url); }
  }
}
