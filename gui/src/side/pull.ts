// On a phone the side lies over the whole page and comes from the screen's
// right side (`app/pulls.ts`): a swipe to the left anywhere on a
// conversation draws it out, and pushed right it goes back as far as it is
// pushed.

import { pulled } from "../app/pulls";

export interface PulledSide {
  /** The desk it lies over, as wide as it travels. */
  host: HTMLElement;
  panel: HTMLElement;
  /** Whether it lies over a page now shown, with something to show. */
  ready(): boolean;
  isOpen(): boolean;
  open(): void;
  close(): void;
}

/** Lets swipes pull the side; what is returned stops them. */
export function followSide(side: PulledSide): () => void {
  const { host, panel } = side;
  return pulled({
    from: "right",
    ready: () => side.ready(),
    isOpen: () => side.isOpen(),
    holds: (target) => panel.contains(target),
    width: () => host.clientWidth,
    take: (opening) => {
      panel.style.transition = "none";
      if (!opening) return;
      // Drawn with what it holds before any of it shows, so it comes with
      // the finger whole.
      panel.style.transform = "translateX(100%)";
      side.open();
    },
    at: (share) => {
      panel.style.transform = `translateX(${((1 - share) * 100).toFixed(2)}%)`;
    },
    leave: (open) => {
      // From where the finger left it, the rest of the way by the side's
      // own movement.
      panel.style.removeProperty("transition");
      if (open && !side.isOpen()) side.open();
      else if (!open && side.isOpen()) side.close();
      panel.style.removeProperty("transform");
    },
  });
}
