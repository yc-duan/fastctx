import { displayedText, textMatches } from "./find-text";

/** Highlight ranges belong to drawn blocks, while match identities belong
 * to the index. Unchanged blocks keep their ranges through a stream. */
export class FindHighlights {
  private query = "";
  private ranges = new Map<HTMLElement, Range[]>();
  private dirty = new Set<HTMLElement>();
  private all: Set<Range> | null = null;
  private chosen: Set<Range> | null = null;
  private current: Range | null = null;
  private identity: string | null = null;
  private frame = 0;
  private readonly observer: MutationObserver;

  constructor(private readonly root: HTMLElement, changed: () => void) {
    this.observer = new MutationObserver((records) => {
      for (const record of records) {
        const element = record.target instanceof Element ? record.target : record.target.parentElement;
        const block = element?.closest<HTMLElement>(".block");
        if (block) this.dirty.add(block);
        for (const node of [...record.addedNodes, ...record.removedNodes]) {
          if (node instanceof HTMLElement && node.classList.contains("block")) this.dirty.add(node);
        }
      }
      if (!this.frame) this.frame = requestAnimationFrame(() => {
        this.frame = 0;
        changed();
      });
    });
  }

  paint(query: string, identity: string | null, current: () => Range | null): void {
    const registry = (CSS as unknown as { highlights?: Map<string, unknown> }).highlights;
    const Highlight = (window as unknown as { Highlight?: new () => Set<Range> }).Highlight;
    if (!registry || !Highlight) return;
    if (!this.all || !this.chosen) {
      this.all = new Highlight();
      this.chosen = new Highlight();
    }
    if (this.root.isConnected && registry.get("find") !== this.all) {
      registry.set("find", this.all);
      registry.set("find-current", this.chosen);
    }
    if (query !== this.query) {
      // Closed or empty search does not follow the stream. Starting again
      // scans the mounted blocks, including changes made while unobserved.
      if (query && !this.query) {
        this.observer.observe(this.root, { childList: true, characterData: true, subtree: true, attributes: true, attributeFilter: ["open", "hidden"] });
      } else if (!query) {
        this.observer.disconnect();
        cancelAnimationFrame(this.frame);
        this.frame = 0;
      }
      this.query = query;
      this.all.clear();
      this.ranges.clear();
      this.dirty.clear();
      if (query) for (const block of this.root.querySelectorAll<HTMLElement>(":scope > .block")) this.dirty.add(block);
    }
    for (const block of this.dirty) {
      for (const range of this.ranges.get(block) ?? []) this.all.delete(range);
      this.ranges.delete(block);
      if (!query || !this.root.contains(block) || !block.childNodes.length) continue;
      const text = displayedText(block);
      const ranges: Range[] = [];
      for (const match of textMatches(text.text, query)) {
        const range = text.range(match);
        if (!range) continue;
        ranges.push(range);
        this.all.add(range);
      }
      this.ranges.set(block, ranges);
    }
    this.dirty.clear();
    if (identity !== this.identity || !this.current?.startContainer.isConnected || !this.current.endContainer.isConnected) {
      this.identity = identity;
      this.current = identity ? current() : null;
      this.chosen.clear();
      if (this.current) this.chosen.add(this.current);
    }
  }

  destroy(): void {
    this.observer.disconnect();
    cancelAnimationFrame(this.frame);
    this.all?.clear();
    this.chosen?.clear();
    this.ranges.clear();
    this.dirty.clear();
  }
}
