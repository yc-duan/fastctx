// The window's own frame on Windows: the shell draws no system titlebar,
// so the page gives the window its caption buttons, in the top right
// corner over the page's head, and says where the window can be dragged
// from: the sidebar's head, each page's head and the row of tabs of a
// conversation's side where it stands at the window's top, apart from what
// can be clicked in them. The native side (`world/titlebar.ts`, the shell's
// titlebar module) keeps Windows' own behaviour: snapping, the system menu,
// double-clicking to maximize, sizing from the edges.
//
// The buttons are drawn with Windows' own icon font, so they look as the
// system's do; hovering the close button turns it red as the system's does.

import { escape } from "../conversation/prose";
import { connectTitlebar, titlebarAvailable, type TitlebarState } from "../world/titlebar";

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

/** Puts the frame in place where the shell has none; `shown` draws it in a
 *  plain browser too, to be looked at. */
export function installFrame(words: FrameWords, shown = false): void {
  const native = titlebarAvailable();
  if (!native && !shown) return;
  document.documentElement.classList.add("framed");
  const controls = document.createElement("div");
  controls.className = "window-controls";
  controls.innerHTML = (["minimize", "maximize", "close"] as const)
    .map(
      (kind) =>
        `<button class="window-control ${kind}" type="button" data-world-titlebar="${kind}" aria-label="${escape(words[kind])}" title="${escape(words[kind])}"><span aria-hidden="true">${GLYPHS[kind]}</span></button>`,
    )
    .join("");
  document.body.append(controls);
  const maximize = controls.querySelector<HTMLButtonElement>(".maximize")!;

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

  if (!native) return;
  let maximized: boolean | null = null;
  const state = (now: TitlebarState) => {
    if (maximized === now.maximized) return;
    maximized = now.maximized;
    const label = now.maximized ? words.restore : words.maximize;
    maximize.firstElementChild!.textContent = now.maximized ? GLYPHS.restore : GLYPHS.maximize;
    maximize.setAttribute("aria-label", label);
    maximize.title = label;
    document.documentElement.classList.toggle("maximized", now.maximized);
  };
  connectTitlebar({
    onState: state,
    // The frame stops following the page; said where it can be seen.
    onError: (error) => console.error("titlebar:", error),
  }).catch((error: unknown) => console.error("titlebar:", error));
}
