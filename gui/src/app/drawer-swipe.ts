// On a phone the sidebar's drawer moves with the finger (`pulls.ts`): a
// swipe to the right anywhere on the page draws it out of the screen's left
// side, and pushed left it goes back as far as it is pushed.

import { pulled } from "./pulls";

export interface Drawer {
  frame: HTMLElement;
  scrim: HTMLElement;
  narrow(): boolean;
  isOpen(): boolean;
  open(): void;
  close(): void;
}

export function followDrawer(drawer: Drawer): void {
  const { frame, scrim } = drawer;
  pulled({
    from: "left",
    ready: () => drawer.narrow(),
    isOpen: () => drawer.isOpen(),
    holds: (target) => frame.contains(target) || scrim.contains(target),
    width: () => frame.offsetWidth,
    take: () => {
      frame.style.transition = "none";
      scrim.style.transition = "none";
      scrim.style.pointerEvents = "auto";
    },
    at: (share) => {
      frame.style.setProperty("--slide", `${((share - 1) * 102).toFixed(2)}%`);
      scrim.style.setProperty("--shown", share.toFixed(3));
    },
    leave: (open) => {
      // From where the finger left it, the rest of the way by the drawer's
      // own movement.
      for (const element of [frame, scrim]) element.style.removeProperty("transition");
      frame.style.removeProperty("--slide");
      scrim.style.removeProperty("--shown");
      scrim.style.removeProperty("pointer-events");
      if (open && !drawer.isOpen()) drawer.open();
      else if (!open && drawer.isOpen()) drawer.close();
    },
  });
}
