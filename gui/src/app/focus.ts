// Parts of the interface are drawn again as the World changes (every two
// seconds, or every three while a card is open). Drawing replaces their
// markup, and with it whatever had keyboard focus; the person moving through
// them with Tab would be thrown back to the top of the page each time. A
// redraw therefore finds the control that had focus again, by what it is
// (its tag, its data attributes, where it links), and gives focus back.

/** Draws `element` again with `draw`, keeping keyboard focus where it was. */
export function redrawKeepingFocus(element: HTMLElement, draw: () => void): void {
  const active = document.activeElement;
  const kept = active instanceof HTMLElement && active !== element && element.contains(active) ? identify(element, active) : null;
  draw();
  if (!kept || element.contains(document.activeElement)) return;
  const again = element.querySelectorAll<HTMLElement>(kept.selector)[kept.index];
  again?.focus({ preventScroll: true });
}

/** A selector that finds this control again after a redraw, and which of
 *  the controls it matches it was. */
function identify(root: HTMLElement, control: HTMLElement): { selector: string; index: number } {
  const parts = [control.tagName.toLowerCase()];
  for (const { name, value } of control.attributes) {
    // The window frame's marks come a frame after a redraw; they name no
    // control.
    if (name.startsWith("data-world-")) continue;
    if (name.startsWith("data-") || name === "href" || name === "name") parts.push(`[${name}="${CSS.escape(value)}"]`);
  }
  // Without anything naming it, a control is known by its class.
  if (parts.length === 1) for (const name of control.classList) parts.push(`.${CSS.escape(name)}`);
  const selector = parts.join("");
  return { selector, index: [...root.querySelectorAll(selector)].indexOf(control) };
}
