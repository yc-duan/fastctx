// The conversations looked at last stay alive (`world-gui.md`, 对话): left
// for another page, a conversation's page keeps following its session and
// keeps everything as it was, what was opened still open; going back shows
// it at once, not built again. The least recently looked at go once more
// than `KEPT` are alive.

import type { Page } from "../app/shell";

/** How many conversations stay alive besides the one in view: the "few
 *  looked at last" the user asked for. A choice, not a measure; each costs
 *  the memory its conversation and what is drawn of it take. */
export const KEPT = 5;

/** A page that can be left without being ended, and shown again. */
export interface KeepablePage extends Page {
  /** It goes out of view, still alive. */
  suspend(): void;
  /** It is in view again. */
  resume(): void;
  /** It ends for good. */
  leave(): void;
}

export class KeptPages {
  /** By key, the one looked at last at the end. */
  private pages = new Map<string, KeepablePage>();

  /** The page for `key`, as it was left when it is alive, else made. The
   *  page handed back ends nothing when left: it is kept. */
  page(key: string, make: () => KeepablePage): Page {
    let page = this.pages.get(key);
    const again = !!page;
    if (page) this.pages.delete(key);
    else page = make();
    this.pages.set(key, page);
    const kept = page;
    if (again) queueMicrotask(() => kept.resume());
    this.trim(key);
    return {
      element: kept.element,
      title: kept.title,
      leave: () => {
        if (this.pages.get(key) === kept) kept.suspend();
      },
    };
  }

  /** Ends the page for `key`, so that it is made anew next time. */
  drop(key: string): void {
    const page = this.pages.get(key);
    if (!page) return;
    this.pages.delete(key);
    page.leave();
  }

  /** Ends the pages looked at longest ago beyond those kept. */
  private trim(current: string): void {
    for (const [key, page] of this.pages) {
      if (this.pages.size <= KEPT + 1) return;
      if (key === current) continue;
      this.pages.delete(key);
      page.leave();
    }
  }
}
