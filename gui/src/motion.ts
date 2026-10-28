// How much the interface moves (`world-gui.md`, 外观): as the system asks,
// always, or not at all. Its one switch, `data-reduced` on the root, is set
// before the first paint (`index.html`) and again whenever the choice or
// the system changes; every rule and script that moves asks this switch,
// never the system on its own. Information that moves, such as a
// conversation's working dots, is not motion it takes away.

export type Motion = "system" | "always" | "off";

export const motions: Motion[] = ["system", "always", "off"];

const KEPT = "motion";
// Asked of the system in a page; none where there is no page, as when the
// wire checks load this in Node.
const asked = typeof matchMedia === "function" ? matchMedia("(prefers-reduced-motion: reduce)") : null;
const listeners = new Set<() => void>();

/** The choice kept on this device; following the system when none is. */
export function chosenMotion(): Motion {
  try {
    const kept = localStorage.getItem(KEPT);
    return kept === "always" || kept === "off" ? kept : "system";
  } catch {
    return "system";
  }
}

/** A CSS time, as a token holds it, in milliseconds. */
export function msOf(time: string): number {
  const value = parseFloat(time);
  if (!Number.isFinite(value)) return 0;
  return time.trim().endsWith("ms") ? value : value * 1000;
}

/** Whether the interface moves less now. */
export function reducedMotion(): boolean {
  const motion = chosenMotion();
  return motion === "off" || (motion === "system" && !!asked?.matches);
}

export function chooseMotion(motion: Motion): void {
  try {
    if (motion === "system") localStorage.removeItem(KEPT);
    else localStorage.setItem(KEPT, motion);
  } catch {
    // Not kept: the choice holds until the window closes.
  }
  apply();
}

/** Calls `listener` whenever whether the interface moves less changes;
 *  answers how to stop. */
export function onMotion(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

function apply(): void {
  if (typeof document === "undefined") return;
  const reduced = reducedMotion();
  const root = document.documentElement;
  if (root.hasAttribute("data-reduced") === reduced) return;
  root.toggleAttribute("data-reduced", reduced);
  for (const listener of listeners) listener();
}

asked?.addEventListener("change", apply);
apply();
