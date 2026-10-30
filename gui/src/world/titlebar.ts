/** Windows native titlebar bridge. Install once after mounting three named
 *  HTML buttons. Mark regions with data-world-titlebar="drag" or "client";
 *  mark the buttons "minimize", "maximize", and "close". Interactive content
 *  inside a drag region must be marked "client" (including its descendants).
 *  Marked elements may change as pages mount. Rectangles are viewport aligned;
 *  split nonrectangular/overlapping regions into their visible rectangles.
 *
 *  The bridge changes no layout or styling. The maximize button receives
 *  data-world-hovered/data-world-pressed because Windows owns its mouse input.
 *  Use onState to update its icon and accessible name (maximize vs restore).
 *  Real buttons retain keyboard and accessibility activation through click.
 *  Do not also bind Tauri drag-region or window actions to these elements.
 */
import { invoke } from "@tauri-apps/api/core";
import { inShell } from "./bridge";
import { listen } from "@tauri-apps/api/event";

export interface TitlebarState {
  maximized: boolean;
  hovered: boolean;
  pressed: boolean;
}

export const titlebarAvailable = (): boolean => inShell() && navigator.userAgent.includes("Windows");

type Rect = { x: number; y: number; width: number; height: number };
type Regions = { drag: Rect[]; client: Rect[]; maximize: Rect | null; scale: number };
type Action = "minimize" | "toggle_maximize" | "close";
const command = <T>(name: string, args?: Record<string, unknown>) => invoke<T>(`plugin:world-titlebar|${name}`, args);
const selector = "[data-world-titlebar]";

type Clip = { horizontal: boolean; vertical: boolean; box: DOMRect };

function rectangles(element: HTMLElement, clips: Map<HTMLElement, Clip | null>): Rect[] {
  if (!element.checkVisibility({ checkOpacity: true, checkVisibilityCSS: true })) return [];
  return Array.from(element.getClientRects(), (rect) => clipped(element, rect, clips))
    .filter((rect): rect is Rect => rect !== null);
}

/** What of a box can be seen: an element's boxes keep their full size
 *  inside an ancestor that clips them (a sidebar folded to no width still
 *  reports its links where they were), so each is cut to its clipping
 *  ancestors. */
function clipped(element: HTMLElement, { left, top, right, bottom }: DOMRect, clips: Map<HTMLElement, Clip | null>): Rect | null {
  for (let holder = element.parentElement; holder && holder !== document.documentElement; holder = holder.parentElement) {
    let clip = clips.get(holder);
    if (clip === undefined) {
      const style = getComputedStyle(holder);
      const horizontal = style.overflowX !== "visible";
      const vertical = style.overflowY !== "visible";
      clip = horizontal || vertical ? { horizontal, vertical, box: holder.getBoundingClientRect() } : null;
      clips.set(holder, clip);
    }
    if (!clip) continue;
    if (clip.horizontal) {
      left = Math.max(left, clip.box.left);
      right = Math.min(right, clip.box.right);
    }
    if (clip.vertical) {
      top = Math.max(top, clip.box.top);
      bottom = Math.min(bottom, clip.box.bottom);
    }
  }
  return right > left && bottom > top ? { x: left, y: top, width: right - left, height: bottom - top } : null;
}

function regions(): Regions {
  const result: Regions = { drag: [], client: [], maximize: null, scale: devicePixelRatio };
  const buttons = new Set<string>();
  // Regions share clipping ancestors; geometry is cached only for this read.
  const clips = new Map<HTMLElement, Clip | null>();
  for (const element of document.querySelectorAll<HTMLElement>(selector)) {
    const kind = element.dataset.worldTitlebar;
    const rects = rectangles(element, clips);
    if (kind === "drag") result.drag.push(...rects);
    else if (kind === "client") result.client.push(...rects);
    else if (kind === "minimize" || kind === "maximize" || kind === "close") {
      if (!(element instanceof HTMLButtonElement)) throw new Error(`The ${kind} titlebar control must be a button`);
      if (buttons.has(kind)) throw new Error(`Duplicate ${kind} titlebar button`);
      buttons.add(kind);
      if (kind === "maximize" && !element.disabled) result.maximize = rects[0] ?? null;
      else result.client.push(...rects);
    } else throw new Error(`Unknown titlebar region: ${String(kind)}`);
  }
  for (const kind of ["minimize", "maximize", "close"]) {
    if (!buttons.has(kind)) throw new Error(`Missing ${kind} titlebar button`);
  }
  // A modal dialog holds the page: nothing behind it drags the window, so a
  // press on the dialog where it lies over a caption reaches the dialog.
  if (document.querySelector("dialog:modal")) {
    result.client.push({ x: 0, y: 0, width: document.documentElement.clientWidth, height: document.documentElement.clientHeight });
  }
  return result;
}

/** Returns an asynchronous disposer that unregisters the input regions.
 *  The Windows shell stays frameless across disposal and page navigation.
 *  Startup failures reject; later failures go to onError and stop geometry
 *  updates. Browser previews, macOS, and mobile should check availability.
 */
export async function connectTitlebar(options: {
  onState: (state: TitlebarState) => void;
  onError: (error: unknown) => void;
}): Promise<() => Promise<void>> {
  if (!titlebarAvailable()) throw new Error("Native World titlebars are available only in the Windows shell");
  let disposed = false;
  let last = "";
  let state: TitlebarState = { maximized: false, hovered: false, pressed: false };
  let sending: Promise<void> | null = null;
  const paintState = () => {
    const maximize = document.querySelector<HTMLButtonElement>('button[data-world-titlebar="maximize"]');
    maximize?.toggleAttribute("data-world-hovered", state.hovered);
    maximize?.toggleAttribute("data-world-pressed", state.pressed);
    options.onState(state);
  };
  const unlistenState = await listen<TitlebarState>("world-titlebar-state", ({ payload }) => {
    if (!disposed) { state = payload; paintState(); }
  });
  let unlistenError: (() => void) | undefined;
  const clicked = (event: MouseEvent) => {
    const target = event.composedPath().find((node): node is HTMLButtonElement =>
      node instanceof HTMLButtonElement && node.matches(selector));
    if (!target || target.disabled) return;
    const kind = target.dataset.worldTitlebar;
    const action: Action | undefined = kind === "maximize" ? "toggle_maximize"
      : kind === "minimize" || kind === "close" ? kind : undefined;
    if (action) { event.preventDefault(); void command("action", { action }).catch(options.onError); }
  };
  const send = async () => {
    const current = regions();
    const signature = JSON.stringify(current);
    if (signature === last) return;
    state = await command<TitlebarState>("update", { regions: current });
    last = signature;
    if (!disposed) paintState();
  };

  // Geometry is sent when it may have changed, never on every frame of an
  // idle window (following every frame kept an idle shell at a tenth of a
  // core). What moves a region: pages mounting and marks changing (DOM
  // mutations), sizes (each marked element and the page), the window's
  // size, browser zoom and the screen's DPI, fonts arriving, and a
  // transition or animation of a region or of what holds one, followed
  // frame by frame only while it runs. IPC occurs only when geometry
  // changes, with one update in flight so an old layout cannot overwrite a
  // new one.
  let queued = 0;
  let again = false;
  const moving = new Map<Element, number>();
  const schedule = () => {
    if (!disposed && !queued) queued = requestAnimationFrame(flush);
  };
  const flush = () => {
    queued = 0;
    if (sending) {
      again = true;
      return;
    }
    // A moving element taken out of the page never says it stopped.
    for (const element of moving.keys()) if (!element.isConnected) moving.delete(element);
    sending = send();
    sending.then(
      () => {
        sending = null;
        if (again || moving.size) {
          again = false;
          schedule();
        }
      },
      (error: unknown) => {
        sending = null;
        options.onError(error);
      },
    );
  };
  const holdsRegion = (element: Element) => element.matches(selector) || element.querySelector(selector) !== null;
  const started = (event: Event) => {
    const element = event.target;
    if (!(element instanceof Element) || !holdsRegion(element)) return;
    moving.set(element, (moving.get(element) ?? 0) + 1);
    schedule();
  };
  const ended = (event: Event) => {
    const element = event.target;
    if (!(element instanceof Element)) return;
    const count = moving.get(element);
    if (count === undefined) return;
    if (count > 1) moving.set(element, count - 1);
    else moving.delete(element);
    schedule();
  };
  const sizes = new ResizeObserver(schedule);
  const observed = new Set<Element>();
  const observeSizes = () => {
    const current = new Set<Element>([document.documentElement, ...document.querySelectorAll(selector)]);
    for (const element of observed) {
      if (current.has(element)) continue;
      sizes.unobserve(element);
      observed.delete(element);
    }
    for (const element of current) {
      if (observed.has(element)) continue;
      sizes.observe(element);
      observed.add(element);
    }
  };
  // Only a change that can add, remove, hide or move a region asks for the
  // geometry again: words streaming into a conversation and rows drawn in it
  // hold no region, and following them kept the shell measuring every frame.
  // A dialog opening or going is such a change too: while one is modal the
  // whole window is the page's (`regions`).
  const regional = `${selector}, dialog`;
  const holdsRegionOrDialog = (element: Element) => element.matches(regional) || element.querySelector(regional) !== null;
  const touchesRegion = (record: MutationRecord) => {
    if (record.type === "attributes") {
      return record.attributeName === "data-world-titlebar" || (record.target instanceof Element && holdsRegionOrDialog(record.target));
    }
    for (const nodes of [record.addedNodes, record.removedNodes]) {
      for (const node of nodes) if (node instanceof Element && holdsRegionOrDialog(node)) return true;
    }
    return false;
  };
  const mutations = new MutationObserver((records) => {
    if (!records.some(touchesRegion)) return;
    observeSizes();
    schedule();
  });
  let resolution: MediaQueryList | null = null;
  const watchResolution = () => {
    resolution?.removeEventListener("change", resolutionChanged);
    resolution = matchMedia(`(resolution: ${devicePixelRatio}dppx)`);
    resolution.addEventListener("change", resolutionChanged);
  };
  const resolutionChanged = () => {
    watchResolution();
    schedule();
  };
  const listening: [EventTarget, string, EventListener][] = [
    [window, "resize", schedule],
    [document, "transitionrun", started],
    [document, "transitionend", ended],
    [document, "transitioncancel", ended],
    [document, "animationstart", started],
    [document, "animationend", ended],
    [document, "animationcancel", ended],
    [document.fonts, "loadingdone", schedule],
  ];
  if (window.visualViewport) listening.push([window.visualViewport, "resize", schedule]);
  try {
    unlistenError = await listen<string>("world-titlebar-error", ({ payload }) => options.onError(new Error(payload)));
    await send();
    document.addEventListener("click", clicked);
    for (const [target, type, listener] of listening) target.addEventListener(type, listener, true);
    mutations.observe(document.body, {
      childList: true,
      subtree: true,
      attributes: true,
      attributeFilter: ["data-world-titlebar", "class", "style", "hidden", "open"],
    });
    observeSizes();
    watchResolution();
  } catch (error) {
    disposed = true;
    unlistenState();
    unlistenError?.();
    await command("detach");
    throw error;
  }
  return async () => {
    disposed = true;
    cancelAnimationFrame(queued);
    document.removeEventListener("click", clicked);
    for (const [target, type, listener] of listening) target.removeEventListener(type, listener, true);
    mutations.disconnect();
    sizes.disconnect();
    observed.clear();
    resolution?.removeEventListener("change", resolutionChanged);
    unlistenState();
    unlistenError?.();
    try { await sending; }
    finally {
      await command("detach");
      for (const element of document.querySelectorAll(selector)) {
        element.removeAttribute("data-world-hovered");
        element.removeAttribute("data-world-pressed");
      }
    }
  };
}
