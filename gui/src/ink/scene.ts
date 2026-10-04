import { Colors } from './colors';
import { tessellate } from './path';
import type { Mesh } from './path';
import { Renderer } from './renderer';
import type { DrawStroke } from './renderer';
import { defaultStyle } from './types';
import type { InkOptions, InkStroke, InkStyle } from './types';

interface Entry extends DrawStroke {
  source: InkStroke;
  mesh: Mesh;
  born: number;
  drawIn: number;
  exit?: { start: number; duration: number };
}

function seedHash(value: string): number {
  let hash = 2166136261;
  for (let i = 0; i < value.length; i++) hash = Math.imul(hash ^ value.charCodeAt(i), 16777619);
  return hash >>> 0;
}

function positive(value: number, name: string, zero = false): void {
  if (!Number.isFinite(value) || (zero ? value < 0 : value <= 0)) throw new RangeError(`${name} must be finite and ${zero ? 'nonnegative' : 'positive'}`);
}

function styleFor(patch: Partial<InkStyle>): InkStyle {
  const style = structuredClone({ ...defaultStyle, ...patch });
  for (const key of ['width', 'fibers', 'density'] as const) positive(style[key], key);
  for (const key of ['spread', 'opacity', 'roughness', 'jitter', 'jitterSpeed'] as const) positive(style[key], key, true);
  if (!Number.isInteger(style.fibers) || style.opacity > 1) throw new RangeError('fibers must be an integer and opacity must be in [0, 1]');
  if (style.dash) {
    positive(style.dash.length, 'dash.length'); positive(style.dash.gap, 'dash.gap', true);
    if (![style.dash.speed, style.dash.offset ?? 0].every(Number.isFinite)) throw new RangeError('Dash speed and offset must be finite');
  }
  return style;
}

/** An owned transparent canvas layer. dispose() releases every listener and GPU allocation. */
export class InkScene {
  private renderer: Renderer;
  private colors: Colors;
  private entries = new Map<string, Entry>();
  private options: InkOptions;
  private resizeObserver: ResizeObserver;
  private intersection: IntersectionObserver;
  private mutations: MutationObserver;
  private motion = matchMedia('(prefers-reduced-motion: reduce)');
  private dprQuery = matchMedia(`(resolution: ${devicePixelRatio}dppx)`);
  private visible = false;
  private lost = false;
  private disposed = false;
  private playing: boolean;
  private raf = 0;
  private last: number | null = null;
  private time = 0;
  private width = 0;
  private height = 0;
  private step: number;
  private tolerance: number;
  private frames = 0;
  private uploads = 0;
  private drawMilliseconds = 0;

  constructor(readonly canvas: HTMLCanvasElement, options: InkOptions = {}) {
    this.options = structuredClone(options);
    this.step = options.quality?.step ?? 8;
    this.tolerance = options.quality?.tolerance ?? 0.25;
    positive(this.step, 'quality.step'); positive(this.tolerance, 'quality.tolerance');
    options.viewBox?.forEach(value => positive(value, 'viewBox'));
    this.playing = options.autoStart !== false;
    this.renderer = new Renderer(canvas);
    this.colors = new Colors(canvas);
    this.resizeObserver = new ResizeObserver(() => this.resize());
    this.resizeObserver.observe(canvas, { box: 'device-pixel-content-box' });
    this.intersection = new IntersectionObserver(entries => {
      this.visible = entries[entries.length - 1].isIntersecting;
      this.reschedule();
    });
    this.intersection.observe(canvas);
    this.mutations = new MutationObserver(this.refreshColors);
    this.observeTheme();
    document.addEventListener('visibilitychange', this.reschedule);
    this.motion.addEventListener('change', this.motionChanged);
    this.dprQuery.addEventListener('change', this.dprChanged);
    canvas.addEventListener('webglcontextlost', this.contextLost);
    canvas.addEventListener('webglcontextrestored', this.contextRestored);
    this.resize();
  }

  add(id: string, stroke: InkStroke, transition: { drawIn?: number } = {}): void {
    this.assertAlive();
    if (this.entries.has(id)) throw new Error(`Duplicate stroke: ${id}`);
    const drawIn = transition.drawIn ?? 0;
    positive(drawIn, 'drawIn', true);
    const source = structuredClone(stroke);
    const style = styleFor(source.style ?? {});
    const color = this.colors.get(style.color);
    const mesh = tessellate(source.path, this.step, this.tolerance);
    const geometry = this.renderer.upload(mesh);
    this.uploads++;
    this.entries.set(id, { source, mesh, geometry, style, color,
      seed: seedHash(`${this.options.seed ?? 0}\0${source.seed ?? id}`),
      length: mesh.length, closed: !!source.path.closed,
      born: this.time, drawIn: this.motion.matches ? 0 : drawIn / 1000,
      reveal: 1, alpha: 1 });
    this.schedule();
  }

  /** Patch one stroke. Style-only changes never tessellate or upload a path. */
  update(id: string, patch: Partial<InkStroke>): void {
    this.assertAlive();
    const entry = this.get(id);
    const next = structuredClone({ ...entry.source, ...patch, style: { ...entry.source.style, ...patch.style } });
    const style = styleFor(next.style);
    const color = this.colors.get(style.color);
    const mesh = patch.path ? tessellate(next.path, this.step, this.tolerance) : entry.mesh;
    if (patch.path) {
      const geometry = this.renderer.upload(mesh);
      this.renderer.release(entry.geometry);
      entry.geometry = geometry; this.uploads++;
    }
    Object.assign(entry, { source: next, mesh, style, color, length: mesh.length, closed: !!next.path.closed,
      seed: seedHash(`${this.options.seed ?? 0}\0${next.seed ?? id}`) });
    this.schedule();
  }

  /** Delete permanently, optionally after a fade measured in active milliseconds. */
  delete(id: string, transition: { fadeOut?: number } = {}): void {
    this.assertAlive();
    const entry = this.get(id), fadeOut = transition.fadeOut ?? 0;
    positive(fadeOut, 'fadeOut', true);
    if (fadeOut && !this.motion.matches) entry.exit = { start: this.time, duration: fadeOut / 1000 };
    else this.destroyEntry(id, entry);
    this.schedule();
  }

  has(id: string): boolean { return this.entries.has(id); }

  /** Call after changing stylesheets or application-specific theme attributes. */
  refreshColors = (): void => {
    this.assertAlive(); this.observeTheme(); this.colors.clear();
    for (const entry of this.entries.values()) entry.color = this.colors.get(entry.style.color);
    this.schedule();
  };

  pause(): void { this.assertAlive(); this.playing = false; this.cancel(); }
  resume(): void { this.assertAlive(); this.playing = true; this.schedule(); }

  /** Deterministic frame in active seconds. Pause first; timestamps must be nonnegative. */
  renderAt(seconds: number): void {
    this.assertAlive(); positive(seconds, 'seconds', true);
    if (this.playing) throw new Error('Pause the scene before calling renderAt');
    if (this.lost) throw new Error('The WebGL context is lost');
    this.time = seconds;
    this.paint();
  }

  get stats() {
    let fibers = 0, vertices = 0, geometryBytes = 0;
    for (const entry of this.entries.values()) {
      fibers += entry.style.fibers;
      vertices += entry.geometry.count * entry.style.fibers;
      geometryBytes += entry.geometry.bytes;
    }
    return { strokes: this.entries.size, fibers, vertices, geometryBytes, uploads: this.uploads,
      frames: this.frames, drawMilliseconds: this.drawMilliseconds, activeSeconds: this.time,
      scheduled: this.raf !== 0, visible: this.visible, reducedMotion: this.motion.matches,
      contextLost: this.lost, width: this.canvas.width, height: this.canvas.height };
  }

  dispose(): void {
    if (this.disposed) return;
    this.cancel();
    this.resizeObserver.disconnect(); this.intersection.disconnect(); this.mutations.disconnect();
    document.removeEventListener('visibilitychange', this.reschedule);
    this.motion.removeEventListener('change', this.motionChanged);
    this.dprQuery.removeEventListener('change', this.dprChanged);
    this.canvas.removeEventListener('webglcontextlost', this.contextLost);
    this.canvas.removeEventListener('webglcontextrestored', this.contextRestored);
    for (const entry of this.entries.values()) this.renderer.release(entry.geometry);
    this.entries.clear(); this.renderer.dispose(); this.colors.dispose();
    this.disposed = true;
  }

  private assertAlive(): void { if (this.disposed) throw new Error('InkScene has been disposed'); }
  private get(id: string): Entry {
    const entry = this.entries.get(id);
    if (!entry) throw new Error(`Unknown stroke: ${id}`);
    return entry;
  }
  private destroyEntry(id: string, entry: Entry): void { this.renderer.release(entry.geometry); this.entries.delete(id); }
  private observeTheme(): void {
    this.mutations.disconnect();
    let element: Element | null = this.canvas;
    while (element) {
      this.mutations.observe(element, { attributes: true, attributeFilter: ['class', 'style', 'data-theme'] });
      // Custom elements inherit variables through their host, beyond the DOM parent chain.
      const root = element.getRootNode();
      element = element.parentElement ?? (root instanceof ShadowRoot ? root.host : null);
    }
  }
  private canRun(): boolean { return this.playing && this.visible && !document.hidden && !this.lost && !this.disposed && this.width > 0 && this.height > 0; }
  private schedule(): void { if (!this.raf && this.canRun()) this.raf = requestAnimationFrame(this.tick); }
  private cancel(): void { cancelAnimationFrame(this.raf); this.raf = 0; this.last = null; }
  private reschedule = (): void => { this.cancel(); this.schedule(); };
  private tick = (now: number): void => {
    this.raf = 0;
    if (!this.canRun()) { this.last = null; return; }
    if (this.last !== null) this.time += (now - this.last) / 1000;
    this.last = now;
    this.paint();
    let animated = false;
    if (!this.motion.matches) for (const entry of this.entries.values()) {
      const dash = entry.style.dash;
      if ((entry.style.jitter > 0 && entry.style.jitterSpeed > 0) || (dash && dash.gap > 0 && dash.speed !== 0) ||
          entry.born + entry.drawIn > this.time || entry.exit) { animated = true; break; }
    }
    if (animated) this.schedule(); else this.last = null;
  };

  private paint(): void {
    if (!this.width || !this.height) return;
    const start = performance.now();
    const staticMotion = this.motion.matches;
    this.renderer.begin(this.canvas.width, this.canvas.height, this.width, this.height, this.options.viewBox, staticMotion ? 0 : this.time);
    for (const [id, entry] of this.entries) {
      entry.reveal = entry.drawIn && !staticMotion ? Math.max(0, Math.min(1, (this.time - entry.born) / entry.drawIn)) : 1;
      entry.alpha = entry.exit ? Math.max(0, 1 - (this.time - entry.exit.start) / entry.exit.duration) : 1;
      if (entry.alpha <= 0) { this.destroyEntry(id, entry); continue; }
      this.renderer.draw(entry, staticMotion ? 0 : this.time);
    }
    this.frames++; this.drawMilliseconds = performance.now() - start;
  }

  private resize = (): void => {
    // client dimensions exclude borders and ignore CSS transforms on an ancestor.
    const style = getComputedStyle(this.canvas);
    this.width = parseFloat(style.width); this.height = parseFloat(style.height);
    const w = Math.round(this.width * devicePixelRatio), h = Math.round(this.height * devicePixelRatio);
    if (this.canvas.width !== w) this.canvas.width = w;
    if (this.canvas.height !== h) this.canvas.height = h;
    this.reschedule();
  };
  private dprChanged = (): void => {
    this.dprQuery.removeEventListener('change', this.dprChanged);
    this.dprQuery = matchMedia(`(resolution: ${devicePixelRatio}dppx)`);
    this.dprQuery.addEventListener('change', this.dprChanged);
    this.resize();
  };
  private motionChanged = (): void => {
    if (this.motion.matches) for (const [id, entry] of this.entries) {
      entry.drawIn = 0;
      if (entry.exit) this.destroyEntry(id, entry);
    }
    this.reschedule();
  };
  private contextLost = (event: Event): void => { event.preventDefault(); this.lost = true; this.cancel(); };
  private contextRestored = (): void => {
    this.renderer = new Renderer(this.canvas);
    for (const entry of this.entries.values()) { entry.geometry = this.renderer.upload(entry.mesh); this.uploads++; }
    this.lost = false; this.reschedule();
  };
}
