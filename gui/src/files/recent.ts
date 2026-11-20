// Recent text viewers keep their original line source until evicted.

import type { FileView } from "./view";

const RECENT_FILES = 3;

export class RecentFileViews {
  private readonly kept = new Map<string, FileView>();

  take(key: string): FileView | undefined {
    const view = this.kept.get(key);
    this.kept.delete(key);
    if (view?.reusable) return view;
    view?.leave();
    return undefined;
  }

  put(key: string, view: FileView): void {
    if (!view.reusable) {
      view.leave();
      return;
    }
    this.kept.get(key)?.leave();
    this.kept.delete(key);
    view.park();
    this.kept.set(key, view);
    if (this.kept.size > RECENT_FILES) {
      const oldest = this.kept.keys().next().value;
      if (oldest !== undefined) {
        this.kept.get(oldest)?.leave();
        this.kept.delete(oldest);
      }
    }
  }

  clear(): void {
    for (const view of this.kept.values()) view.leave();
    this.kept.clear();
  }
}
