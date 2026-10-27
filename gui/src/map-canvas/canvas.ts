import { CanvasCamera, type CameraOptions } from "./camera";
import { DropTargets, nearestItem, type ValidTarget } from "./drop";
import { placeLabels } from "./labels";
import { onMotion, reducedMotion } from "../motion";
import { inputOf, pressFelt } from "../app/press";
import type { Bounds, CanvasEvent, CanvasFrame, CanvasHandle, CanvasItem, Draft, Point } from "./types";
import "./canvas.css";

export interface CanvasOptions extends CameraOptions {
  label: string;
  controls: { minus: string; plus: string; fit: string; scale: string };
  /** The hint under the drawing for a mouse: how to zoom and how to move
   *  it. */
  /** The hints under the drawing, and what follows them on that line, such
   *  as a way to what the drawing is for. */
  hints?: { zoom: string; pan: string; after?: HTMLElement } | undefined;
  /** A live predicate: final release always checks it again. */
  validTarget: ValidTarget;
  event(event: CanvasEvent): void;
  /** Called once per paint; rebuild page detail only when view.level changes. */
  frame?(frame: CanvasFrame): void;
  /** Text for assistive technology, supplied by the page. */
  draftLabel?(draft: Draft): string;
  /** A picture to look at: always fitted to its frame, never moved or
   *  zoomed, with no controls; its items can still be chosen. */
  still?: boolean;
  /** The smallest scale the drawing fits itself at, so its names can be
   *  read; asked for its whole (the fit control), it fits however small. */
  fitFloor?: number;
  /** The largest scale the drawing fits itself at: a small one is not
   *  blown up past its own size. */
  fitMost?: number;
  /** Whether the canvas writes the items' names itself, placed clear of
   *  each other; a page that writes them on its drawing does without. */
  names?: boolean;
}

/** A pointer down on the drawing. With a mouse the left button chooses
 *  or, dragged, draws a frame round several; the middle button moves the
 *  drawing. A finger or a pen moves it. */
interface Pointer { point: Point; start: Point; item: string | null; moved: boolean; touch: boolean; middle?: boolean; frame?: boolean; add?: boolean }
interface ButtonLabel { item: CanvasItem; button: HTMLButtonElement; label: HTMLElement | null; width: number; height: number }

/** Items this far outside the view, in pixels, already have their hit areas,
 *  so a drag reaches them before they come in. */
const HIT_MARGIN = 240;

/** The page owns ink and operations. This surface owns the camera, input, hit
 * objects and plain HTML names. Dispose it when its page leaves the screen. */
export class MapCanvas {
  readonly element = document.createElement("figure");
  /** What lies on the paper under the ink, and over it. They move with the
   *  ink but are not scaled with it: the page draws them to the camera's
   *  scale itself, as SVG text under a scaled HTML layer is painted, in
   *  Chrome on Android, at the scale it was last laid out at. Neither is
   *  blended with the paper. */
  readonly under = document.createElement("div");
  readonly over = document.createElement("div");
  /** The ink: the page may blend it with the paper. */
  readonly ink = document.createElement("div");
  readonly overlay = document.createElement("div");
  readonly camera: CanvasCamera;
  private readonly abort = new AbortController();
  private readonly targets: DropTargets;
  private readonly names = document.createElement("div");
  private readonly hits = document.createElement("div");
  private readonly status = document.createElement("span");
  private readonly control = document.createElement("div");
  private readonly scaleText = document.createElement("output");
  private readonly marquee = document.createElement("div");
  private readonly resize: ResizeObserver;
  private readonly buttons = new Map<string, ButtonLabel>();
  private readonly handles = new Map<string, { handle: CanvasHandle; button: HTMLButtonElement }>();
  private items: readonly CanvasItem[] = [];
  private itemIndex = new Map<string, CanvasItem>();
  private bounds: Bounds = { x: 0, y: 0, width: 0, height: 0 };
  private pointers = new Map<number, Pointer>();
  private pointed: string | null = null;
  private selected: string | null = null;
  private focused: string | null = null;
  private draft: Draft | null = null;
  private draftPointer: number | null = null;
  private animation = 0;
  private disposed = false;
  private longPress = 0;
  private longPressed = false;
  /** Where a finger's hold began, on which item, while it may still become a long press. */
  private held: { item: string; point: Point } | null = null;
  private rect = this.element.getBoundingClientRect();
  private fitOnResize = true;
  /** Whether the whole drawing was asked for, however small that makes it. */
  private fitWhole = false;
  /** What a drawing too large to read whole shows first (`setScene`). */
  private focus: Bounds | null = null;
  private controlWidth = 0;
  /** From the controls' top to the frame's foot, while they are shown. */
  private controlRoom = 0;
  private labelObstacles: readonly Bounds[] = [];
  private frameState: CanvasFrame;
  private previousView = { x: NaN, y: NaN, scale: NaN };

  constructor(private readonly options: CanvasOptions) {
    this.camera = new CanvasCamera(options);
    this.targets = new DropTargets(options.validTarget);
    this.frameState = { view: this.camera.view, moving: false, reducedMotion: reducedMotion(), hiddenLabels: [] };
    this.element.className = options.still ? "map-canvas map-still" : "map-canvas";
    this.element.tabIndex = options.still ? -1 : 0;
    this.element.setAttribute("role", "group");
    this.element.setAttribute("aria-label", options.label);
    this.ink.className = "canvas-ink";
    this.ink.setAttribute("aria-hidden", "true");
    for (const [layer, name] of [[this.under, "canvas-under"], [this.over, "canvas-over"]] as const) {
      layer.className = name;
      layer.setAttribute("aria-hidden", "true");
    }
    this.overlay.className = "canvas-overlay";
    this.names.className = "canvas-names";
    this.names.setAttribute("aria-hidden", "true");
    this.hits.className = "canvas-hits";
    this.control.className = "canvas-controls";
    this.status.className = "canvas-status";
    this.status.setAttribute("aria-live", "polite");
    this.status.setAttribute("aria-atomic", "true");
    this.scaleText.setAttribute("aria-label", options.controls.scale);
    for (const [text, label, action] of [
      ["−", options.controls.minus, () => this.zoom(1 / 1.25)],
      ["+", options.controls.plus, () => this.zoom(1.25)],
      ["↔", options.controls.fit, () => this.fit()],
    ] as const) {
      const button = document.createElement("button");
      button.type = "button";
      button.textContent = text;
      button.setAttribute("aria-label", label);
      button.title = label;
      button.addEventListener("click", action, { signal: this.abort.signal });
      this.control.append(button);
      if (text === "−") this.control.append(this.scaleText);
    }
    this.marquee.className = "canvas-marquee";
    this.marquee.hidden = true;
    this.overlay.append(this.marquee);
    this.element.append(this.under, this.ink, this.over, this.overlay, this.hits, this.names, this.status);
    if (!options.still) this.element.append(this.control);
    if (options.hints && !options.still) {
      const hints = document.createElement("div");
      hints.className = "canvas-hints";
      hints.innerHTML = `<span class="canvas-hint" aria-hidden="true"><kbd>Ctrl</kbd><span class="canvas-plus">+</span>${MOUSE}<span>${escapeHtml(options.hints.zoom)}</span></span><span class="canvas-hint" aria-hidden="true">${MOUSE}<span>${escapeHtml(options.hints.pan)}</span></span>`;
      if (options.hints.after) hints.append(options.hints.after);
      this.element.append(hints);
    }
    const signal = this.abort.signal;
    this.element.addEventListener("pointerdown", e => this.down(e), { signal });
    this.element.addEventListener("pointermove", e => this.move(e), { signal });
    this.element.addEventListener("pointerup", e => this.up(e), { signal });
    this.element.addEventListener("pointercancel", e => this.cancelPointer(e.pointerId), { signal });
    this.element.addEventListener("lostpointercapture", e => { if (this.pointers.has(e.pointerId)) this.cancelPointer(e.pointerId); }, { signal });
    this.element.addEventListener("pointerleave", () => { if (!this.pointers.size) this.point(null, [0, 0]); }, { signal });
    this.element.addEventListener("wheel", e => this.wheel(e), { passive: false, signal });
    // Pressed twice, an item opens; the paper fits the drawing to the view.
    this.element.addEventListener("dblclick", e => {
      if (options.still || (e.target as Element).closest("[data-handle]") || this.isControl(e.target)) return;
      const key = this.keyOf(e.target) ?? this.itemAt(this.local(e));
      e.preventDefault();
      if (key) this.options.event({ kind: "open", key });
      else this.fit();
    }, { signal });
    this.element.addEventListener("contextmenu", e => {
      if (this.isControl(e.target)) return;
      e.preventDefault();
      // A finger's long press comes when the phone's own does: the hold
      // begun on an item ends now, not at the drawing's own interval.
      if (inputOf(e) === "touch") { if (this.longPress) this.holdEnded(); return; }
      if (!this.longPressed) this.options.event({ kind: "context", key: this.itemAt(this.local(e)), at: this.local(e), input: "mouse" });
    }, { signal });
    this.element.addEventListener("keydown", e => this.key(e), { signal });
    this.element.addEventListener("click", e => {
      // Pointer selection was settled on pointerup after the pan threshold.
      if (e.detail !== 0 || this.isControl(e.target)) return;
      const target = e.target as HTMLElement;
      const handle = this.handles.get(target.closest<HTMLElement>("[data-handle]")?.dataset.handle ?? "");
      if (handle) this.beginDraft(handle.handle, this.camera.toScreen(handle.handle.point), true);
      else if (this.keyOf(target)) this.select(this.keyOf(target), this.camera.toScreen(this.buttons.get(this.keyOf(target)!)!.item.point), e.ctrlKey || e.metaKey);
    }, { signal });
    this.element.addEventListener("focusin", e => {
      const key = this.keyOf(e.target);
      this.focused = key;
      const item = key ? this.buttons.get(key)?.item : undefined;
      const handle = this.handles.get((e.target as HTMLElement).dataset.handle ?? "")?.handle;
      if (item || handle) { this.camera.reveal((item ?? handle)!.point, 100); this.point(key, this.camera.toScreen((item ?? handle)!.point)); this.schedule(); }
    }, { signal });
    this.element.addEventListener("focusout", e => {
      if (!this.element.contains(e.relatedTarget as Node | null)) { this.focused = null; if (this.draft?.keyboard) this.cancelDraft("interrupted"); this.schedule(); }
    }, { signal });
    window.addEventListener("blur", () => { this.clearPointers(); this.cancelDraft("interrupted"); }, { signal });
    const unfollow = onMotion(() => this.schedule());
    signal.addEventListener("abort", unfollow);
    document.fonts.addEventListener("loadingdone", () => { this.measure(); this.schedule(); }, { signal });
    this.resize = new ResizeObserver(() => {
      this.rect = this.element.getBoundingClientRect();
      this.camera.resize(this.element.clientWidth, this.element.clientHeight);
      this.measure();
      if (this.fitOnResize) this.camera.fit(this.bounds, this.fitPadding(), this.floor(), this.focus, this.most(), this.fitBelow());
      this.schedule();
    });
    this.resize.observe(this.element);
  }

  get frame(): CanvasFrame { return this.frameState; }
  get activeDraft(): Draft | null { return this.draft; }
  get selection(): string | null { return this.selected; }
  /** Mirror the page's selection without emitting a user gesture. */
  setSelection(key: string | null): void {
    if (key && !this.itemIndex.has(key)) throw new Error("Unknown canvas selection");
    this.selected = key;
    this.schedule();
  }

  /** Brings the drawing to `scale`, its origin at `x`, `y` on the screen, as
   *  a page flying to a part of it does frame by frame. */
  view(scale: number, x: number, y: number): void {
    this.fitOnResize = false;
    this.camera.set({ ...this.camera.view, scale, x, y });
    this.schedule();
  }
  /** Screen-space areas occupied by the page's floating cards or legends. */
  reserveLabels(boxes: readonly Bounds[]): void {
    if (JSON.stringify(boxes) === JSON.stringify(this.labelObstacles)) return;
    this.labelObstacles = boxes.map(box => ({ ...box }));
    this.schedule();
  }
  /** The measured size of an item's name, shown or withheld. */
  labelSize(key: string): { width: number; height: number } | null {
    const entry = this.buttons.get(key);
    return entry ? { width: entry.width, height: entry.height } : null;
  }
  /** CSS-pixel centre of a grip, kept clear of its source's hit area at far zoom. */
  handlePoint(key: string): Point | null {
    const handle = this.handles.get(key)?.handle;
    if (!handle) return null;
    const point = this.camera.toScreen(handle.point);
    const source = this.buttons.get(handle.source)?.item;
    if (!source) return point;
    const start = this.camera.toScreen(source.point);
    const distance = Math.hypot(point[0] - start[0], point[1] - start[1]);
    const reach = Math.max(22, source.radius * this.camera.view.scale) + 22 + 4;
    if (distance >= reach) return point;
    return [start[0] + (distance ? (point[0] - start[0]) / distance : 1) * reach,
      start[1] + (distance ? (point[1] - start[1]) / distance : 0) * reach];
  }

  /** Reconcile by identity; focusing a machine survives a detail redraw. */
  /** What the drawing holds. With `keepView`, the drawing stays where it is
   *  and is no longer fitted to what it holds: a change the person made in
   *  it, such as folding a part, must not move the rest under the pointer. */
  /** `focus` is what of a drawing too large to read whole is shown first. */
  setScene(items: readonly CanvasItem[], handles: readonly CanvasHandle[], bounds: Bounds, keepView = false, focus: Bounds | null = null): void {
    const index = new Map(items.map(i => [i.key, i]));
    if (index.size !== items.length || new Set(handles.map(h => h.key)).size !== handles.length) throw new Error("Duplicate canvas identity");
    this.items = items;
    this.itemIndex = index;
    this.bounds = bounds;
    this.focus = focus;
    for (const [key, entry] of this.buttons) {
      const item = index.get(key);
      if (!item) { entry.button.remove(); entry.label?.remove(); this.buttons.delete(key); }
      else this.dress(entry, item);
    }
    // A canvas that writes the names has a hit area for every item from the
    // start; one that does not makes them as the items come into view.
    if (this.options.names !== false) for (const item of items) this.entryOf(item);
    const handleKeys = new Set(handles.map(h => h.key));
    for (const [key, entry] of this.handles) { if (!handleKeys.has(key)) { entry.button.remove(); this.handles.delete(key); } }
    for (const handle of handles) {
      let entry = this.handles.get(handle.key);
      if (!entry) {
        const button = document.createElement("button");
        button.type = "button";
        button.className = "canvas-handle";
        button.dataset.handle = handle.key;
        button.textContent = "·";
        this.hits.append(button);
        entry = { handle, button };
        this.handles.set(handle.key, entry);
      }
      entry.handle = handle;
      entry.button.setAttribute("aria-label", handle.label);
      entry.button.title = handle.label;
    }
    if (this.selected && !index.has(this.selected)) this.select(null, [0, 0]);
    if (this.pointed && !index.has(this.pointed)) this.point(null, [0, 0]);
    // Until the drawing is moved by hand it stays fitted to what it holds.
    if (keepView) this.fitOnResize = false;
    else if (this.fitOnResize && this.camera.view.width > 0) this.camera.fit(this.bounds, this.fitPadding(), this.floor(), this.focus, this.most(), this.fitBelow());
    if (this.draft) {
      const current = this.handles.get(this.draft.handle.key)?.handle;
      if (!current || current.source !== this.draft.handle.source || current.point.some((n, i) => n !== this.draft!.handle.point[i])) this.cancelDraft("changed");
      else this.refreshDraft(this.camera.toScreen(this.draft.point));
    }
    this.measure();
    this.schedule();
  }
  /** The hit area of `item`, made when first wanted, with its name where
   *  the canvas writes names. */
  private entryOf(item: CanvasItem): ButtonLabel {
    let entry = this.buttons.get(item.key);
    if (!entry) {
      const button = document.createElement("button");
      button.type = "button";
      button.dataset.item = item.key;
      button.className = "canvas-object";
      let label: HTMLElement | null = null;
      if (this.options.names !== false) {
        label = document.createElement("span");
        label.className = "canvas-name";
        label.hidden = true;
        label.dataset.label = item.key;
        this.names.append(label);
      }
      this.hits.append(button);
      entry = { item, button, label, width: 0, height: 0 };
      this.buttons.set(item.key, entry);
      this.dress(entry, item);
    }
    return entry;
  }

  /** What a hit area says of its item, kept up as the item changes. */
  private dress(entry: ButtonLabel, item: CanvasItem): void {
    entry.item = item;
    const said = [item.label, item.sub].filter(Boolean).join(" ");
    if (entry.button.getAttribute("aria-label") !== said) entry.button.setAttribute("aria-label", said);
    if (entry.button.dataset.kind !== item.kind) entry.button.dataset.kind = item.kind;
    if (!entry.label) return;
    entry.label.dataset.side = item.side ?? "";
    const text = [item.label, item.sub].filter(Boolean).join("\n");
    if (entry.label.textContent !== text) entry.label.textContent = text;
  }
  pan(dx: number, dy: number): void { this.fitOnResize = false; this.camera.pan(dx, dy); this.schedule(); }
  zoom(factor: number, at?: Point): void { this.fitOnResize = false; this.camera.zoom(factor, at); this.schedule(); }
  /** The whole drawing in view, however small, kept so as it changes. */
  fit(): void { this.fitOnResize = true; this.fitWhole = true; this.camera.fit(this.bounds, this.fitPadding(), 0, this.focus, this.most(), this.fitBelow()); this.schedule(); }
  /** The scale the drawing fits itself no larger than. */
  private most(): number {
    return this.options.fitMost ?? Infinity;
  }
  /** The scale the drawing fits itself no smaller than. */
  private floor(): number {
    return this.fitWhole || this.options.still ? 0 : (this.options.fitFloor ?? 0);
  }
  /** The zoom boundaries the page reads its levels by, from now on. */
  levels(levels: readonly number[]): void { this.camera.setLevels(levels); this.schedule(); }
  /** Use after directly changing the camera, or changing CSS font metrics. */
  refresh(): void { this.measure(); this.schedule(); }
  dispose(): void {
    this.disposed = true;
    this.clearPointers();
    this.cancelDraft("interrupted");
    this.abort.abort();
    this.resize.disconnect();
    cancelAnimationFrame(this.animation);
  }
  /** Room left round the world when it is fitted: a tenth of the shorter
   *  side, from 24 to 76 pixels; on a phone's narrow frame its gutter, so
   *  the drawing is not shrunk away. */
  private fitPadding(): number {
    const { width, height } = this.camera.view;
    return width < 480 ? 16 : Math.max(24, Math.min(76, Math.min(width, height) * 0.1));
  }
  /** Room left under the drawing when it is fitted: its padding, or what
   *  the controls in its corner take, and a little air over them. */
  fitBelow(): number {
    return Math.max(this.fitPadding(), this.controlRoom);
  }
  private measure(): void {
    this.controlWidth = this.control.offsetWidth;
    this.controlRoom = this.control.isConnected && this.control.offsetHeight ? this.element.clientHeight - this.control.offsetTop + 8 : 0;
    if (this.options.names === false) return;
    const labels = [...this.buttons.values()].flatMap(e => (e.label ? [e.label] : []));
    const visibility = new Map(labels.map(label => [label, label.hidden]));
    for (const label of labels) label.hidden = false;
    for (const entry of this.buttons.values()) { entry.width = entry.label?.offsetWidth ?? 0; entry.height = entry.label?.offsetHeight ?? 0; }
    for (const [label, hidden] of visibility) label.hidden = hidden;
  }
  private schedule(): void {
    if (this.animation || this.disposed) return;
    this.animation = requestAnimationFrame(() => { this.animation = 0; this.paint(); });
  }
  private paint(): void {
    const view = this.camera.view;
    const changed = view.x !== this.previousView.x || view.y !== this.previousView.y || view.scale !== this.previousView.scale;
    const moving = this.pointers.size > 0 || changed;
    this.previousView = view;
    this.ink.style.transform = `translate(${view.x}px,${view.y}px) scale(${view.scale})`;
    for (const layer of [this.under, this.over]) layer.style.transform = `translate(${view.x}px,${view.y}px)`;
    const obstacles: Bounds[] = [...this.labelObstacles];
    const writes = this.options.names !== false;
    // Where a canvas does not write names it keeps hit areas only for what
    // is in view or about to be, and the one chosen, pointed at or focused.
    if (!writes) {
      const [left, top] = this.camera.toWorld([-HIT_MARGIN, -HIT_MARGIN]);
      const [right, bottom] = this.camera.toWorld([view.width + HIT_MARGIN, view.height + HIT_MARGIN]);
      const kept = (key: string) => key === this.selected || key === this.pointed || key === this.focused;
      for (const item of this.items) {
        const box = item.box;
        const reach = item.radius;
        const seen = box
          ? box.x + box.width >= left && box.x <= right && box.y + box.height >= top && box.y <= bottom
          : item.point[0] + reach >= left && item.point[0] - reach <= right && item.point[1] + reach >= top && item.point[1] - reach <= bottom;
        if ((seen && !item.quiet) || kept(item.key)) this.entryOf(item);
        else {
          const entry = this.buttons.get(item.key);
          if (entry) { entry.button.remove(); this.buttons.delete(item.key); }
        }
      }
    }
    for (const entry of this.buttons.values()) {
      const [x, y] = this.camera.toScreen(entry.item.point);
      const box = entry.item.box;
      if (box) {
        // A rectangle as it is drawn, at least a finger's height.
        const [left, top] = this.camera.toScreen([box.x, box.y]);
        const width = box.width * view.scale;
        const height = Math.max(44, box.height * view.scale);
        const shift = (height - box.height * view.scale) / 2;
        entry.button.style.transform = `translate(${left}px,${top - shift}px)`;
        entry.button.style.width = `${width}px`;
        entry.button.style.height = `${height}px`;
        entry.button.style.borderRadius = box.corners ? box.corners.map((corner) => `${corner * view.scale}px`).join(" ") : "";
      } else {
        const size = Math.max(44, entry.item.radius * view.scale * 2);
        entry.button.style.transform = `translate(${x - size / 2}px,${y - size / 2}px)`;
        entry.button.style.width = entry.button.style.height = `${size}px`;
      }
      const pressed = String(entry.item.key === this.selected);
      if (entry.button.getAttribute("aria-pressed") !== pressed) entry.button.setAttribute("aria-pressed", pressed);
      entry.button.dataset.valid = String(this.draft?.valid.includes(entry.item.key) ?? false);
      entry.button.dataset.target = String(this.draft?.target === entry.item.key);
      entry.button.dataset.pointed = String(this.pointed === entry.item.key);
      if (!writes) continue;
      const r = (entry.item.labelRadius ?? entry.item.radius) * view.scale;
      obstacles.push({ x: x - r, y: y - r, width: 2 * r, height: 2 * r });
    }
    for (const { handle, button } of this.handles.values()) {
      const [x, y] = this.handlePoint(handle.key)!;
      button.style.transform = `translate(${x - 22}px,${y - 22}px)`;
    }
    const hiddenLabels: string[] = [];
    if (writes) {
      // Reserve the corner control; CSS positions it without per-frame layout reads.
      obstacles.push({ x: view.width - this.controlWidth - 12, y: view.height - 56, width: this.controlWidth + 12, height: 56 });
      const placements = placeLabels([...this.buttons.values()].map(e => ({ key: e.item.key, point: this.camera.toScreen(e.item.point),
        radius: (e.item.labelRadius ?? e.item.radius) * view.scale, width: e.width, height: e.height,
        priority: e.item.key === this.focused || e.item.key === this.pointed ? 100 : e.item.key === this.selected ? 50 : e.item.priority ?? 0, side: e.item.side })),
        { x: 8, y: 8, width: Math.max(0, view.width - 16), height: Math.max(0, view.height - 16) }, obstacles);
      for (const { key, box } of placements) {
        const label = this.buttons.get(key)!.label!;
        label.hidden = !box;
        if (box) label.style.transform = `translate(${box.x}px,${box.y}px)`;
        else hiddenLabels.push(key);
      }
    }
    this.scaleText.textContent = `${Math.round(view.scale * 100)}%`;
    this.element.dataset.moving = String(moving);
    this.element.dataset.panning = String([...this.pointers.values()].some((pointer) => pointer.middle || (!pointer.frame && pointer.moved)));
    this.element.dataset.reducedMotion = String(reducedMotion());
    this.frameState = { view, moving, reducedMotion: reducedMotion(), hiddenLabels };
    this.options.frame?.(this.frameState);
    if (changed) this.schedule();
  }
  private keyOf(target: EventTarget | null): string | null { return target instanceof Element ? target.closest<HTMLElement>("[data-item]")?.dataset.item ?? null : null; }
  private itemAt(point: Point): string | null { return nearestItem(this.items, point, p => this.camera.toScreen(p), this.camera.view.scale)?.key ?? null; }
  private isControl(target: EventTarget | null): boolean { return target instanceof Node && (this.control.contains(target) || this.overlay.contains(target)); }
  private local(e: { clientX: number; clientY: number }): Point { return [e.clientX - this.rect.left, e.clientY - this.rect.top]; }
  private point(key: string | null, at: Point): void {
    if (this.pointed === key) return;
    this.pointed = key;
    this.options.event({ kind: "point", key, at });
    this.schedule();
  }
  private select(key: string | null, at: Point, add = false): void {
    this.selected = key;
    this.options.event({ kind: "select", key, at, add });
    this.schedule();
  }
  private stopLongPress(): void { clearTimeout(this.longPress); this.longPress = 0; this.held = null; }
  /** A finger held on an item long enough: it is chosen, and its menu asked for. */
  private holdEnded(): void {
    const held = this.held;
    this.stopLongPress();
    if (!held) return;
    this.longPressed = true;
    this.select(held.item, held.point);
    this.options.event({ kind: "context", key: held.item, at: held.point, input: "touch" });
    pressFelt();
  }
  private clearPointers(): void {
    this.stopLongPress();
    for (const id of this.pointers.keys()) { if (this.element.hasPointerCapture(id)) this.element.releasePointerCapture(id); }
    this.pointers.clear();
    this.schedule();
  }
  private down(e: PointerEvent): void {
    // The middle button moves the paper and never chooses or opens
    // anything; with a mouse the left button chooses, or draws a frame.
    const middle = e.button === 1;
    if ((e.button !== 0 && !middle) || this.isControl(e.target) || (middle && this.options.still)) return;
    const frame = e.pointerType === "mouse" && !middle && !this.options.still;
    e.preventDefault();
    // Capture keyboard cancellation throughout a pointer gesture without moving
    // the camera as focusing an off-screen item would do.
    this.element.focus({ preventScroll: true });
    this.rect = this.element.getBoundingClientRect();
    const point = this.local(e);
    this.longPressed = false;
    this.stopLongPress();
    const item = middle ? null : this.itemAt(point);
    this.pointers.set(e.pointerId, { point, start: point, item, moved: false, touch: e.pointerType === "touch", middle, frame, add: e.ctrlKey || e.metaKey });
    this.element.setPointerCapture(e.pointerId);
    if (middle) {
      this.cancelDraft("interrupted");
    } else if (this.pointers.size === 1) {
      const handleKey = (e.target as Element).closest<HTMLElement>("[data-handle]")?.dataset.handle;
      const handle = handleKey ? this.handles.get(handleKey) : undefined;
      if (handle) { this.beginDraft(handle.handle, point, false); this.draftPointer = e.pointerId; }
      else {
        this.cancelDraft("interrupted");
        if (e.pointerType === "touch" && item) {
          // 500 ms is the gesture's deliberate hold interval, not an operation
          // timeout; the system's own long press ends it sooner where it comes.
          this.held = { item, point };
          this.longPress = window.setTimeout(() => this.holdEnded(), 500);
        }
      }
    } else {
      this.cancelDraft("interrupted");
      for (const pointer of this.pointers.values()) pointer.moved = true;
    }
    this.schedule();
  }
  private move(e: PointerEvent): void {
    const p = this.local(e);
    const pointer = this.pointers.get(e.pointerId);
    if (!pointer) { if (e.pointerType !== "touch") this.point(this.isControl(e.target) ? null : this.itemAt(p), p); return; }
    const previous = [...this.pointers.values()].slice(0, 2).map(p => p.point);
    const old = pointer.point;
    pointer.point = p;
    if (Math.hypot(p[0] - pointer.start[0], p[1] - pointer.start[1]) >= (pointer.touch ? 8 : 5)) {
      pointer.moved = true;
      this.stopLongPress();
    }
    if (this.draft && this.draftPointer === e.pointerId) { this.refreshDraft(p); return; }
    if (this.options.still) return;
    if (this.pointers.size >= 2) {
      const now = [...this.pointers.values()].slice(0, 2).map(p => p.point);
      const centre = (points: Point[]): Point => [(points[0][0] + points[1][0]) / 2, (points[0][1] + points[1][1]) / 2];
      const before = centre(previous), after = centre(now);
      const distance = (points: Point[]) => Math.hypot(points[0][0] - points[1][0], points[0][1] - points[1][1]);
      if (distance(previous) > 0) this.camera.zoom(distance(now) / distance(previous), before);
      this.pan(after[0] - before[0], after[1] - before[1]);
    } else if (pointer.frame) {
      if (pointer.moved) this.drawFrame(pointer.start, p, false, !!pointer.add);
    } else if ((pointer.moved || pointer.middle) && !this.longPressed) this.pan(p[0] - old[0], p[1] - old[1]);
  }
  /** The frame from `from` to `to`, on the screen, and the items inside. */
  private drawFrame(from: Point, to: Point, done: boolean, add: boolean): void {
    const x = Math.min(from[0], to[0]);
    const y = Math.min(from[1], to[1]);
    const width = Math.abs(to[0] - from[0]);
    const height = Math.abs(to[1] - from[1]);
    this.marquee.hidden = done;
    this.marquee.style.transform = `translate(${x}px,${y}px)`;
    this.marquee.style.width = `${width}px`;
    this.marquee.style.height = `${height}px`;
    const keys = this.items
      .filter((item) => {
        const [sx, sy] = this.camera.toScreen(item.point);
        return sx >= x && sx <= x + width && sy >= y && sy <= y + height;
      })
      .map((item) => item.key);
    const [left, top] = this.camera.toWorld([x, y]);
    const [right, bottom] = this.camera.toWorld([x + width, y + height]);
    this.options.event({ kind: "marquee", keys, area: { x: left, y: top, width: right - left, height: bottom - top }, done, add });
  }
  private up(e: PointerEvent): void {
    const pointer = this.pointers.get(e.pointerId);
    if (!pointer) return;
    // A browser can coalesce the last move into release. Apply that final position
    // before deciding whether this was a click, pan, pinch or handle drop.
    this.move(e);
    this.stopLongPress();
    if (this.draft && this.draftPointer === e.pointerId) { this.refreshDraft(this.local(e)); this.finishDraft(this.local(e)); }
    else if (pointer.frame && pointer.moved) this.drawFrame(pointer.start, this.local(e), true, !!pointer.add);
    else if (!pointer.middle && !pointer.moved && !this.longPressed && this.pointers.size === 1) this.select(pointer.item, this.local(e), !!pointer.add);
    this.pointers.delete(e.pointerId);
    if (this.element.hasPointerCapture(e.pointerId)) this.element.releasePointerCapture(e.pointerId);
    this.schedule();
  }
  private cancelPointer(id: number): void {
    this.stopLongPress();
    if (this.pointers.get(id)?.frame) this.marquee.hidden = true;
    this.pointers.delete(id);
    if (this.draftPointer === id) this.cancelDraft("interrupted");
    this.schedule();
  }
  /** Ctrl and the wheel zoom (a touchpad's pinch comes as the same); the
   *  wheel alone moves nothing. */
  private wheel(e: WheelEvent): void {
    if (this.options.still || this.isControl(e.target) || !(e.ctrlKey || e.metaKey)) return;
    e.preventDefault();
    this.rect = this.element.getBoundingClientRect();
    const unit = e.deltaMode === WheelEvent.DOM_DELTA_LINE ? parseFloat(getComputedStyle(this.element).fontSize) : e.deltaMode === WheelEvent.DOM_DELTA_PAGE ? this.camera.view.height : 1;
    this.zoom(Math.exp(-e.deltaY * unit / 300), this.local(e));
    if (this.draft && !this.draft.keyboard) this.refreshDraft(this.local(e));
  }
  private beginDraft(handle: CanvasHandle, screen: Point, keyboard: boolean): void {
    this.cancelDraft("interrupted");
    this.draft = { handle, point: this.camera.toWorld(screen), target: null, valid: this.targets.list(handle, this.items).map(i => i.key), keyboard };
    this.reportDraft();
  }
  private refreshDraft(screen: Point): void {
    if (!this.draft) return;
    const valid = this.targets.list(this.draft.handle, this.items).map(i => i.key);
    const target = this.targets.at(this.draft.handle, this.items, screen, p => this.camera.toScreen(p), this.camera.view.scale)?.key ?? null;
    this.draft = { ...this.draft, point: this.camera.toWorld(screen), valid, target };
    this.reportDraft();
  }
  private reportDraft(): void {
    if (!this.draft) return;
    this.options.event({ kind: "draft", draft: this.draft });
    if (this.options.draftLabel) this.status.textContent = this.options.draftLabel(this.draft);
    this.schedule();
  }
  private finishDraft(at: Point): void {
    if (!this.draft) return;
    const draft = this.draft;
    const target = this.items.find(i => i.key === draft.target);
    if (!target || !this.options.validTarget(draft.handle, target)) { this.cancelDraft("outside"); return; }
    this.draft = null;
    this.draftPointer = null;
    this.status.textContent = "";
    this.options.event({ kind: "drop", handle: draft.handle, target: target.key, at });
    this.schedule();
  }
  private cancelDraft(reason: "escape" | "outside" | "interrupted" | "changed"): void {
    if (!this.draft) return;
    const handle = this.draft.handle;
    this.draft = null;
    this.draftPointer = null;
    this.status.textContent = "";
    this.options.event({ kind: "cancel", handle, reason });
    this.schedule();
  }
  private key(e: KeyboardEvent): void {
    if (this.isControl(e.target)) return;
    if (e.key === "Escape") { this.cancelDraft("escape"); this.clearPointers(); e.preventDefault(); return; }
    const key = this.keyOf(e.target);
    if (e.key === "ContextMenu" || e.key === "F10" && e.shiftKey) {
      e.preventDefault();
      const item = key ? this.buttons.get(key)?.item : undefined;
      this.options.event({ kind: "context", key, at: item ? this.camera.toScreen(item.point) : [this.camera.view.width / 2, this.camera.view.height / 2], input: "keyboard" });
      return;
    }
    if (this.draft?.keyboard) {
      if (e.key === "Enter" || e.key === " ") { e.preventDefault(); this.finishDraft(this.camera.toScreen(this.draft.point)); return; }
      if (e.key === "Tab" || e.key.startsWith("Arrow")) {
        e.preventDefault();
        const valid = this.targets.list(this.draft.handle, this.items);
        const index = valid.findIndex(i => i.key === this.draft!.target);
        const direction = e.shiftKey || e.key === "ArrowLeft" || e.key === "ArrowUp" ? -1 : 1;
        const item = valid[(index + direction + valid.length) % valid.length];
        if (item) {
          this.camera.reveal(item.point, 100);
          this.draft = { ...this.draft, point: item.point, target: item.key, valid: valid.map(i => i.key) };
          this.reportDraft();
        }
        return;
      }
    }
    if (e.key === "Enter" || e.key === " ") {
      const handle = this.handles.get((e.target as HTMLElement).dataset.handle ?? "")?.handle;
      if (handle) { e.preventDefault(); this.beginDraft(handle, this.camera.toScreen(handle.point), true); return; }
      const item = key ? this.buttons.get(key)?.item : undefined;
      if (item) { e.preventDefault(); this.select(item.key, this.camera.toScreen(item.point), e.ctrlKey || e.metaKey); return; }
    }
    if (this.options.still) return;
    const pans: Record<string, Point> = { ArrowLeft: [48, 0], ArrowRight: [-48, 0], ArrowUp: [0, 48], ArrowDown: [0, -48] };
    if (pans[e.key]) { e.preventDefault(); this.pan(...pans[e.key]); }
    else if (e.key === "+" || e.key === "=") { e.preventDefault(); this.zoom(1.25); }
    else if (e.key === "-" || e.key === "_") { e.preventDefault(); this.zoom(1 / 1.25); }
    else if (e.key === "0") { e.preventDefault(); this.fit(); }
  }
}

/** A mouse, drawn small, its wheel filled: what the hints under the drawing
 *  point at. */
const MOUSE = `<svg class="canvas-mouse" viewBox="0 0 12 18" aria-hidden="true"><rect x="1" y="1" width="10" height="16" rx="5" fill="none" stroke="currentColor" stroke-width="1.2"/><line x1="6" y1="1.5" x2="6" y2="7" stroke="currentColor" stroke-width="0.9"/><rect class="canvas-mouse-wheel" x="4.9" y="3.4" width="2.2" height="3.6" rx="1.1"/></svg>`;

function escapeHtml(text: string): string {
  return text.replace(/[&<>"]/g, (character) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[character]!);
}
