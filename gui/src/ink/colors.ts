/** Resolve CSS colors only when the cascade changes, never in the animation loop. */
export class Colors {
  private probe = document.createElement('span');
  private context = document.createElement('canvas').getContext('2d', { willReadFrequently: true })!;
  private cache = new Map<string, readonly number[]>();

  constructor(canvas: HTMLCanvasElement) {
    this.probe.hidden = true;
    this.probe.setAttribute('aria-hidden', 'true');
    canvas.append(this.probe);
    this.context.canvas.width = this.context.canvas.height = 1;
  }

  get(value: string): readonly number[] {
    const cached = this.cache.get(value);
    if (cached) return cached;
    if (!CSS.supports('color', value)) throw new TypeError(`Invalid ink color: ${value}`);
    // Let the CSS cascade resolve nested fallbacks; unused fallbacks need not exist.
    this.probe.style.setProperty('--ink-probe-color', value);
    const resolved = getComputedStyle(this.probe).getPropertyValue('--ink-probe-color').trim();
    if (!resolved || !CSS.supports('color', resolved)) throw new TypeError(`Unresolved ink color: ${value}`);
    this.probe.style.color = resolved;
    const color = getComputedStyle(this.probe).color;
    this.context.clearRect(0, 0, 1, 1);
    this.context.fillStyle = color;
    this.context.fillRect(0, 0, 1, 1);
    const rgba = Array.from(this.context.getImageData(0, 0, 1, 1).data, n => n / 255);
    this.cache.set(value, rgba);
    return rgba;
  }

  clear(): void { this.cache.clear(); }
  dispose(): void { this.probe.remove(); this.cache.clear(); }
}
