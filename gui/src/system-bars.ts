// The phone's status and navigation bars. The Android shell lays the page
// out under them, edge to edge, and gives it their heights as `--safe-top`
// and `--safe-bottom` (`index.html` reads them before the first paint, the
// shell sets them again as they change). In return the page tells the shell
// its theme, so the window behind the page is that colour from the next
// start's first frame, and whether what lies under the bars is light, for
// the colour of their icons. Elsewhere there is no shell to tell.

import type { Theme } from "./theme";

interface WorldWindow {
  paint(theme: Theme, chosen: boolean, lightUnderBars: boolean, darkCover: boolean): void;
}

const shell = (window as { WorldWindow?: WorldWindow }).WorldWindow;
let painted: { theme: Theme; chosen: boolean } | null = null;
/** How many things that are dark whatever the theme cover the page. */
let darkOver = 0;

function tell(): void {
  if (!shell || !painted) return;
  shell.paint(painted.theme, painted.chosen, painted.theme === "warm-white" && darkOver === 0, darkOver > 0);
}

/** The theme the page is in, and whether the user chose it rather than
 *  following the system. */
export function paintBars(theme: Theme, chosen: boolean): void {
  painted = { theme, chosen };
  tell();
}

/** Something dark covers the page edge to edge, whatever the theme (a
 *  picture opened over it): the bars' icons turn light until the returned
 *  release is called. */
export function darkUnderBars(): () => void {
  darkOver += 1;
  tell();
  let held = true;
  return () => {
    if (!held) return;
    held = false;
    darkOver -= 1;
    tell();
  };
}

/** How far down the page the status bar reaches, in CSS pixels. */
export function safeTop(): number {
  return parseFloat(getComputedStyle(document.documentElement).getPropertyValue("--safe-top"));
}

/** How far up the page the navigation bar reaches, in CSS pixels. */
export function safeBottom(): number {
  return parseFloat(getComputedStyle(document.documentElement).getPropertyValue("--safe-bottom"));
}
