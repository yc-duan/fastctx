// Back, however it is asked for: the page head's back, a mouse's back
// button and Alt+← on a computer, the system back on a phone. It closes
// what lies over the page, topmost first, as Esc does on a computer; with
// nothing over it, a page the user came into goes back to the one they came
// from (`world-gui.md`, 界面的结构). Whatever opens over the page says so
// here as it opens; a modal <dialog> is found by itself. On each back the
// Android shell asks `__worldBack`; when it did nothing, the shell goes back
// a page, and from the first page it steps the app aside.

/** Something over the page that back closes. */
export interface Layer {
  /** Whether it is still open: one closed some other way is passed over. */
  isOpen(): boolean;
  close(): void;
  /** Whether `target` is part of it, for a gesture on it that closes it. */
  holds?(target: Element): boolean;
}

/** The layers in the order they opened, the last on top. */
const stack: Layer[] = [];

/** How the page shown is left by back; none on the sidebar's own pages. */
let pageBack: (() => void) | null = null;

/** Puts `layer` on top, as it opens. */
export function layerOpened(layer: Layer): void {
  for (let at = stack.length - 1; at >= 0; at -= 1) if (stack[at] === layer || !stack[at]!.isOpen()) stack.splice(at, 1);
  stack.push(layer);
}

/** Closes the topmost open layer; whether there was one. A layer that
 *  declines to close, a sheet still at work, takes the back all the same. */
export function closeTopmost(): boolean {
  for (let top = stack.pop(); top; top = stack.pop()) {
    if (!top.isOpen()) continue;
    // Put back first: a layer that stays open stays on top.
    stack.push(top);
    top.close();
    const at = stack.indexOf(top);
    if (!top.isOpen() && at >= 0) stack.splice(at, 1);
    return true;
  }
  return false;
}

/** Closes the open layer `target` is part of, and first whatever opened
 *  over it (a hint, a pop-up of its own); whether it closed. Nothing closes
 *  when no layer holds it. */
export function closeLayerOf(target: Element): boolean {
  let at = stack.length - 1;
  while (at >= 0 && !(stack[at]!.isOpen() && stack[at]!.holds?.(target))) at -= 1;
  if (at < 0) return false;
  const owner = stack[at]!;
  for (const over of stack.slice(at + 1).reverse()) if (over.isOpen()) over.close();
  owner.close();
  for (let i = stack.length - 1; i >= 0; i -= 1) if (!stack[i]!.isOpen()) stack.splice(i, 1);
  return !owner.isOpen();
}

/** Says how the page now shown is left by back, or that it has no back. */
export function setPageBack(back: (() => void) | null): void {
  pageBack = back;
}

/** Leaves the page shown for the one the user came from; whether it has a
 *  back. */
export function leavePage(): boolean {
  if (!pageBack) return false;
  pageBack();
  return true;
}

/** Back: what lies over the page first, else the page; whether it did
 *  anything. */
export function goBack(): boolean {
  return closeTopmost() || leavePage();
}

/** A modal dialog closes as Esc closes it: `cancel` first, which it may
 *  refuse, then `close`. */
function cancel(dialog: HTMLDialogElement): void {
  const requesting = dialog as HTMLDialogElement & { requestClose?: () => void };
  if (typeof requesting.requestClose === "function") requesting.requestClose();
  else if (dialog.dispatchEvent(new Event("cancel", { cancelable: true }))) dialog.close();
}

/** Answers back from now on: every modal dialog shown becomes a layer as it
 *  opens. A mouse's back button and Alt+← never walk the window's own
 *  history: on a page without a back they do nothing. */
export function installBack(): void {
  const layers = new WeakMap<HTMLDialogElement, Layer>();
  new MutationObserver((changes) => {
    for (const change of changes) {
      const dialog = change.target;
      if (!(dialog instanceof HTMLDialogElement) || !dialog.open || change.oldValue !== null) continue;
      let layer = layers.get(dialog);
      if (!layer) {
        layer = { isOpen: () => dialog.open && dialog.isConnected, close: () => cancel(dialog), holds: (target) => dialog.contains(target) };
        layers.set(dialog, layer);
      }
      layerOpened(layer);
    }
  }).observe(document.documentElement, { subtree: true, attributes: true, attributeFilter: ["open"], attributeOldValue: true });
  addEventListener("mousedown", (event) => {
    if (event.button === 3) event.preventDefault();
  });
  addEventListener("mouseup", (event) => {
    if (event.button !== 3) return;
    event.preventDefault();
    goBack();
  });
  addEventListener("keydown", (event) => {
    if (event.key !== "ArrowLeft" || !event.altKey || event.ctrlKey || event.metaKey || event.shiftKey) return;
    event.preventDefault();
    goBack();
  });
  (window as { __worldBack?: () => boolean }).__worldBack = goBack;
  (window as { WorldWindow?: { ready(): void } }).WorldWindow?.ready();
}
