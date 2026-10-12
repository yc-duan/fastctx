// The system back on a phone: it closes what lies over the page, topmost
// first, before the page itself is left, as Esc does on a computer.
// Whatever opens over the page says so here as it opens; a modal <dialog>
// is found by itself. On each back the Android shell asks `__worldBack`;
// when it closed nothing, the shell goes back a page, and from the first
// page it steps the app aside.

/** Something over the page that back closes. */
export interface Layer {
  /** Whether it is still open: one closed some other way is passed over. */
  isOpen(): boolean;
  close(): void;
}

/** The layers in the order they opened, the last on top. */
const stack: Layer[] = [];

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

/** A modal dialog closes as Esc closes it: `cancel` first, which it may
 *  refuse, then `close`. */
function cancel(dialog: HTMLDialogElement): void {
  const requesting = dialog as HTMLDialogElement & { requestClose?: () => void };
  if (typeof requesting.requestClose === "function") requesting.requestClose();
  else if (dialog.dispatchEvent(new Event("cancel", { cancelable: true }))) dialog.close();
}

/** Answers the shell's back from now on: every modal dialog shown becomes
 *  a layer as it opens. */
export function installBack(): void {
  const layers = new WeakMap<HTMLDialogElement, Layer>();
  new MutationObserver((changes) => {
    for (const change of changes) {
      const dialog = change.target;
      if (!(dialog instanceof HTMLDialogElement) || !dialog.open || change.oldValue !== null) continue;
      let layer = layers.get(dialog);
      if (!layer) {
        layer = { isOpen: () => dialog.open && dialog.isConnected, close: () => cancel(dialog) };
        layers.set(dialog, layer);
      }
      layerOpened(layer);
    }
  }).observe(document.documentElement, { subtree: true, attributes: true, attributeFilter: ["open"], attributeOldValue: true });
  (window as { __worldBack?: () => boolean }).__worldBack = closeTopmost;
  (window as { WorldWindow?: { ready(): void } }).WorldWindow?.ready();
}
