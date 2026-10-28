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
  /** Brings one of its turns into view, once it holds it. */
  reach?(turn: string): void;
}

export class KeptPages {
  /** By key, the one looked at last at the end. */
  private pages = new Map<string, KeepablePage>();
  /** Those alive out of view. */
  private away = new Set<string>();
  /** Those whose conversation was deleted while in view: they end when
   *  left. */
  private ending = new Set<string>();

  /** The page for `key`, as it was left when it is alive, else made. The
   *  page handed back ends nothing when left: it is kept. */
  page(key: string, make: () => KeepablePage): Page {
    let page = this.pages.get(key);
    const again = !!page;
    if (page) this.pages.delete(key);
    else page = make();
    this.pages.set(key, page);
    this.away.delete(key);
    const kept = page;
    if (again) queueMicrotask(() => kept.resume());
    this.trim(key);
    return {
      element: kept.element,
      title: kept.title,
      roomWillChange: (by) => kept.roomWillChange?.(by),
      leave: () => {
        if (this.pages.get(key) !== kept) return;
        if (this.ending.has(key)) this.drop(key);
        else {
          kept.suspend();
          this.away.add(key);
        }
      },
    };
  }

  /** Brings a turn of the page for `key` into view, once it is shown. */
  reach(key: string, turn: string): void {
    const page = this.pages.get(key);
    if (page?.reach) queueMicrotask(() => page.reach!(turn));
  }

  /** Ends the page for `key`, so that it is made anew next time. */
  drop(key: string): void {
    const page = this.pages.get(key);
    this.away.delete(key);
    this.ending.delete(key);
    if (!page) return;
    this.pages.delete(key);
    page.leave();
  }

  /** The conversation of `key` was deleted: its page stays alive no longer
   *  than it is in view. */
  gone(key: string): void {
    if (!this.pages.has(key)) return;
    if (this.away.has(key)) this.drop(key);
    else this.ending.add(key);
  }

  /** Ends the pages looked at longest ago beyond those kept. */
  private trim(current: string): void {
    for (const [key] of this.pages) {
      if (this.pages.size <= KEPT + 1) return;
      if (key === current) continue;
      this.drop(key);
    }
  }
}
