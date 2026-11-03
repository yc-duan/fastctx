// The window's own frame: the shell draws no titlebar of its own, so the
// page is the window's top. Where the window can be dragged from is the
// sidebar's head, each page's head and the row of tabs of a conversation's
// side where it stands at the window's top, apart from what can be clicked
// in them; double-clicking there maximizes it.
//
// On Windows the page gives the window its caption buttons, in the top right
// corner over the page's head, drawn with Windows' own icon font so they look
// as the system's do, and the native side (`world/titlebar.ts`, the shell's
// titlebar module) keeps Windows' own behaviour: snapping, the system menu,
// sizing from the edges. On Linux the same buttons are drawn as lines, and
// the page moves the window itself; the shell sizes it from its edges. On
// macOS the system's own buttons stand over the sidebar's head
// (`tauri.macos.conf.json`), and the page moves the window.

import { getCurrentWindow } from "@tauri-apps/api/window";
import { escape } from "../conversation/prose";
import { connectTitlebar, shellSystem, type FrameSystem, type TitlebarState } from "../world/titlebar";

export interface FrameWords {
  minimize: string;
  maximize: string;
  restore: string;
  close: string;
}

/** Where the window is dragged from. */
const CAPTIONS = '.page-head, .sidebar-head, .sp-split:not([data-dock="bottom"]) .sp-head';
/** What can be clicked inside a caption, and so is not dragged from. */
const CLICKABLE = 'a[href], button, input, textarea, select, [tabindex]:not([tabindex="-1"]), [role="button"]';

// Segoe Fluent Icons (Windows 11) and Segoe MDL2 Assets (Windows 10) name
// the caption glyphs alike.
const GLYPHS = { minimize: "", maximize: "", restore: "", close: "" };

// The same four, drawn as lines where the system has no such font.
const LINES = {
  minimize: '<svg viewBox="0 0 10 10" width="10" height="10"><path d="M0 5.5h10"/></svg>',
  maximize: '<svg viewBox="0 0 10 10" width="10" height="10"><rect x="0.5" y="0.5" width="9" height="9"/></svg>',
  restore: '<svg viewBox="0 0 10 10" width="10" height="10"><rect x="0.5" y="2.5" width="7" height="7"/><path d="M2.5 2.5v-2h7v7h-2"/></svg>',
  close: '<svg viewBox="0 0 10 10" width="10" height="10"><path d="M0.5 0.5l9 9M9.5 0.5l-9 9"/></svg>',
};

/** The frame a page asked for with `?frame` is drawn in a plain browser
 *  too, to be looked at: Windows' unless it names another. */
export function previewSystem(asked: string | null): FrameSystem {
  return asked === "macos" || asked === "linux" ? asked : "windows";
}

/** Puts the frame in place where the shell draws none; `preview` draws one
 *  in a plain browser, to be looked at. */
export function installFrame(words: FrameWords, preview: FrameSystem | null = null): void {
  const system = preview ?? shellSystem();
  if (!system) return;
  const live = preview === null;
  document.documentElement.dataset.frame = system;
  if (live && system !== "windows") movesItself();
  if (system === "macos") {
    if (!live) document.body.append(lights());
    return;
  }
  const native = live && system === "windows";
  const glyph = (kind: keyof typeof GLYPHS) =>
    system === "windows" ? GLYPHS[kind] : LINES[kind];
  document.documentElement.classList.add("framed");
  const controls = document.createElement("div");
  controls.className = "window-controls";
  controls.innerHTML = (["minimize", "maximize", "close"] as const)
    .map(
      (kind) =>
        `<button class="window-control ${kind}" type="button" data-world-titlebar="${kind}" aria-label="${escape(words[kind])}" title="${escape(words[kind])}"><span aria-hidden="true">${glyph(kind)}</span></button>`,
    )
    .join("");
  document.body.append(controls);
  const maximize = controls.querySelector<HTMLButtonElement>(".maximize")!;
  let maximized: boolean | null = null;
  const state = (now: TitlebarState) => {
    if (maximized === now.maximized) return;
    maximized = now.maximized;
    const label = now.maximized ? words.restore : words.maximize;
    maximize.firstElementChild!.innerHTML = glyph(now.maximized ? "restore" : "maximize");
    maximize.setAttribute("aria-label", label);
    maximize.title = label;
    document.documentElement.classList.toggle("maximized", now.maximized);
  };
  if (live && system === "linux") {
    const window = getCurrentWindow();
    controls.addEventListener("click", (event) => {
      const kind = (event.target as Element).closest<HTMLElement>(".window-control")?.dataset.worldTitlebar;
      if (kind === "minimize") void window.minimize();
      else if (kind === "maximize") void window.toggleMaximize();
      else if (kind === "close") void window.close();
    });
    const follow = () =>
      void window.isMaximized().then((now) => state({ maximized: now, hovered: false, pressed: false }));
    follow();
    void window.onResized(follow);
    return;
  }
  if (!native) return;

  // Captions are marked as pages draw them, and again as they draw anew.
  let marking = 0;
  const mark = () => {
    marking = 0;
    const captions = new Set(document.querySelectorAll<HTMLElement>(CAPTIONS));
    // One that is no longer a caption, as the side's row moved down from
    // the window's top, is let go.
    for (const was of document.querySelectorAll<HTMLElement>('[data-world-titlebar="drag"]')) {
      if (!captions.has(was)) delete was.dataset.worldTitlebar;
    }
    for (const caption of captions) {
      if (caption.dataset.worldTitlebar !== "drag") caption.dataset.worldTitlebar = "drag";
      for (const clickable of caption.querySelectorAll<HTMLElement>(CLICKABLE)) {
        if (!clickable.dataset.worldTitlebar) clickable.dataset.worldTitlebar = "client";
      }
    }
  };
  mark();
  // A title that becomes renamable gains a tabindex: it is then clicked,
  // no longer dragged from. A side moved to the window's foot no longer
  // has its row at the top.
  // Only what adds, removes or changes a caption, or what is clicked in one,
  // marks them again; what streams into a conversation does not.
  const inCaption = (node: Node) =>
    node instanceof Element && (node.matches(CAPTIONS) || node.querySelector(CAPTIONS) !== null || node.closest(CAPTIONS) !== null);
  const touchesCaption = (record: MutationRecord) =>
    record.type === "attributes"
      ? record.attributeName === "data-dock" || inCaption(record.target)
      : [...record.addedNodes, ...record.removedNodes].some(inCaption) || inCaption(record.target);
  new MutationObserver((records) => {
    if (records.some(touchesCaption)) marking ||= requestAnimationFrame(mark);
  }).observe(document.body, { childList: true, subtree: true, attributes: true, attributeFilter: ["tabindex", "data-dock"] });

  connectTitlebar({
    onState: state,
    // The frame stops following the page; said where it can be seen.
    onError: (error) => console.error("titlebar:", error),
  }).catch((error: unknown) => console.error("titlebar:", error));
}

/** Where the system leaves moving the window to the page (macOS, Linux):
 *  a press on a caption, apart from what can be clicked in it, moves it, and
 *  a double one maximizes it, as the system's own titlebar would. */
function movesItself(): void {
  const window = getCurrentWindow();
  document.addEventListener("mousedown", (event) => {
    if (event.button !== 0 || !(event.target instanceof Element)) return;
    if (!event.target.closest(CAPTIONS) || event.target.closest(CLICKABLE)) return;
    if (event.detail === 2) void window.toggleMaximize();
    else void window.startDragging();
  });
}

/** The three buttons macOS draws at the window's top left, as they stand
 *  there, for a page looked at in a plain browser. */
function lights(): HTMLElement {
  const lights = document.createElement("div");
  lights.className = "frame-lights";
  lights.setAttribute("aria-hidden", "true");
  lights.innerHTML = "<i></i><i></i><i></i>";
  return lights;
}
