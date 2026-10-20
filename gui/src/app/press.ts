// A long press, as the system counts it (`world-gui.md`, 界面的结构).
//
// A phone raises its own long press after the delay its user set, and tells
// the page with a `contextmenu` from a finger; that event is the long press
// here. A timer of the page's own would race it and lose: the WebView answers
// a finger held on a link by dragging the link, and on text by selecting it,
// and either takes the touch away from the page, timer and all, at the very
// moment the system's long press comes. What answers a long press carries
// `data-own-press`; while the last pointer down is a finger, the root says so
// (`data-input`), and `press.css` keeps whatever is so marked from starting a
// drag or a selection.

export type Input = "touch" | "mouse";

interface Haptics {
  press?(): void;
}

/** A press ended in a long press: the click it may end in is not a tap. */
let swallow = false;

/** Follows what the last pointer down was, and swallows the click a long
 *  press may end in. Once, as the interface starts. */
export function installPresses(): void {
  const root = document.documentElement;
  document.addEventListener(
    "pointerdown",
    (event) => {
      swallow = false;
      const input: Input = event.pointerType === "mouse" ? "mouse" : "touch";
      if (root.dataset.input !== input) root.dataset.input = input;
    },
    { capture: true, passive: true },
  );
  document.addEventListener(
    "click",
    (event) => {
      if (!swallow) return;
      swallow = false;
      event.preventDefault();
      event.stopPropagation();
    },
    { capture: true },
  );
}

/** What a `contextmenu` came from: a finger's long press, or the right
 *  button (or the keyboard, taken as the pointer last used). */
export function inputOf(event: MouseEvent): Input {
  const type = (event as PointerEvent).pointerType;
  if (type) return type === "mouse" ? "mouse" : "touch";
  return document.documentElement.dataset.input === "touch" ? "touch" : "mouse";
}

/** Calls `held` when what matches `selector` inside `root` is held by a
 *  finger or opened with the right button. When it answers (anything but
 *  false), the system's own menu and the click the press ends in give way,
 *  and a finger feels the press. Text being typed keeps the system's own. */
export function onHeld(root: HTMLElement, selector: string, held: (element: HTMLElement, event: MouseEvent, input: Input) => boolean | void): void {
  root.addEventListener("contextmenu", (event) => {
    if (event.defaultPrevented) return;
    const target = event.target as HTMLElement;
    if (target.closest("input, textarea, [contenteditable]")) return;
    const element = target.closest<HTMLElement>(selector);
    if (!element || !root.contains(element)) return;
    const input = inputOf(event);
    if (held(element, event, input) === false) return;
    event.preventDefault();
    if (input === "touch") pressFelt();
  });
}

/** A finger's long press was answered: the phone gives the small knock it
 *  gives its own (through the shell, which needs no permission to), and the
 *  click the press ends in is swallowed. */
export function pressFelt(): void {
  swallow = true;
  const shell = (window as { WorldWindow?: Haptics }).WorldWindow;
  if (typeof shell?.press === "function") shell.press();
  else navigator.vibrate?.(8);
}
