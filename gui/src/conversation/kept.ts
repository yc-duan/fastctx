// The conversations looked at last stay alive (`world-gui.md`, 对话): left
// for another page, or taken off the desk, a conversation's page keeps
// following its session and keeps everything as it was, what was opened
// still open; shown again, it is there at once, not built again. Those shown
// on the desk are never let go; of the rest, the least recently looked at go
// once more than `KEPT` wait.

import type { Page } from "../app/shell";

/** How many conversations stay alive besides those in view: the "few looked
 *  at last" the user asked for. A choice, not a measure; each costs the
 *  memory its conversation and what is drawn of it take. */
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

export class KeptPages<T extends KeepablePage = KeepablePage> {
  /** By key, the one looked at last at the end. */
  private pages = new Map<string, T>();
  /** Those alive out of view. */
  private away = new Set<string>();
  /** Those whose conversation was deleted while in view: they end when
   *  taken out of view. */
  private ending = new Set<string>();

  /** The page for `key`, as it was left when it is alive, else made; in
   *  view now, and so kept whatever else comes. */
  take(key: string, make: () => T): T {
    let page = this.pages.get(key);
    const again = !!page && this.away.has(key);
    if (page) this.pages.delete(key);
    else page = make();
    this.pages.set(key, page);
    this.away.delete(key);
    const kept = page;
    if (again) queueMicrotask(() => kept.resume());
    return page;
  }

  /** The page for `key` is out of view: it waits, still alive, among those
   *  kept, unless its conversation was deleted. */
  release(key: string): void {
    const page = this.pages.get(key);
    if (!page || this.away.has(key)) return;
    if (this.ending.has(key)) {
      this.drop(key);
      return;
    }
    page.suspend();
    this.away.add(key);
    this.trim();
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

  /** Ends every page, as when an account is logged out of this device. */
  dropAll(): void {
    for (const key of [...this.pages.keys()]) this.drop(key);
  }

  /** The conversation of `key` was deleted: its page stays alive no longer
   *  than it is in view. */
  gone(key: string): void {
    if (!this.pages.has(key)) return;
    if (this.away.has(key)) this.drop(key);
    else this.ending.add(key);
  }

  /** Ends the pages out of view looked at longest ago, beyond those kept. */
  private trim(): void {
    const waiting = [...this.pages.keys()].filter((key) => this.away.has(key));
    for (const key of waiting.slice(0, Math.max(0, waiting.length - KEPT))) this.drop(key);
  }
}
