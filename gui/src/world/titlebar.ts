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
import { invoke, isTauri } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export interface TitlebarState {
  maximized: boolean;
  hovered: boolean;
  pressed: boolean;
}

export const titlebarAvailable = (): boolean => isTauri() && navigator.userAgent.includes("Windows");

type Rect = { x: number; y: number; width: number; height: number };
type Regions = { drag: Rect[]; client: Rect[]; maximize: Rect | null; scale: number };
type Action = "minimize" | "toggle_maximize" | "close";
const command = <T>(name: string, args?: Record<string, unknown>) => invoke<T>(`plugin:world-titlebar|${name}`, args);
const selector = "[data-world-titlebar]";

function rectangles(element: HTMLElement): Rect[] {
  if (!element.checkVisibility({ checkOpacity: true, checkVisibilityCSS: true })) return [];
  return Array.from(element.getClientRects(), ({ x, y, width, height }) => ({ x, y, width, height }))
    .filter(({ width, height }) => width > 0 && height > 0);
}

function regions(): Regions {
  const result: Regions = { drag: [], client: [], maximize: null, scale: devicePixelRatio };
  const buttons = new Set<string>();
  for (const element of document.querySelectorAll<HTMLElement>(selector)) {
    const kind = element.dataset.worldTitlebar;
    const rects = rectangles(element);
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
  let animation = 0;
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
  // A frame sample also follows ancestor motion, transitions, scrolling,
  // browser zoom and per-monitor DPI. IPC occurs only when geometry changes,
  // with one update in flight so an old layout cannot overwrite a new one.
  const sample = () => {
    if (disposed) return;
    sending = send();
    void sending.then(() => { if (!disposed) animation = requestAnimationFrame(sample); }, options.onError);
  };
  try {
    unlistenError = await listen<string>("world-titlebar-error", ({ payload }) => options.onError(new Error(payload)));
    await send();
    document.addEventListener("click", clicked);
    animation = requestAnimationFrame(sample);
  } catch (error) {
    disposed = true;
    unlistenState();
    unlistenError?.();
    await command("detach");
    throw error;
  }
  return async () => {
    disposed = true;
    cancelAnimationFrame(animation);
    document.removeEventListener("click", clicked);
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
