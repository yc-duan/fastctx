import { textMatches, type TextMatch } from "./find-text";

export interface FindTarget {
  block: string;
  row?: string;
  selector?: string;
  long?: number;
}

export interface FindPart {
  key: string;
  stamp: string;
  target: FindTarget;
  read(signal: AbortSignal): string | Promise<string>;
}

export interface FindChanges {
  order: readonly string[];
  changed: { key: string; parts: Iterable<FindPart> }[];
  removed: string[];
}

interface Document extends FindPart {
  text: string;
  hits: TextMatch[];
}

export interface FindMatch extends TextMatch {
  document: Document;
}

/** Display text and query hits outlive DOM virtualization. Only changed
 * parts are read and searched again during a stream. */
export class FindIndex {
  private blocks = new Map<string, { parts: Document[]; count: number }>();
  private order: readonly string[] = [];
  private query = "";
  private readonly lifetime = new AbortController();
  total = 0;

  async update(changes: FindChanges): Promise<void> {
    this.order = changes.order;
    for (const key of changes.removed) {
      this.total -= this.blocks.get(key)?.count ?? 0;
      this.blocks.delete(key);
    }
    // The browser's frame interval sets the indexing budget, including
    // displays faster than 60 Hz. Streams normally update one part only.
    const frame = await this.frameInterval();
    let began = performance.now();
    for (const block of changes.changed) {
      const old = this.blocks.get(block.key);
      const known = new Map(old?.parts.map((part) => [part.key, part]));
      const parts: Document[] = [];
      let count = 0;
      for (const part of block.parts) {
        this.lifetime.signal.throwIfAborted();
        let document = known.get(part.key);
        if (!document || document.stamp !== part.stamp) {
          const read = part.read(this.lifetime.signal);
          const text = typeof read === "string" ? read : await read;
          this.lifetime.signal.throwIfAborted();
          document = { ...part, text, hits: textMatches(text, this.query) };
        } else document.target = part.target;
        parts.push(document);
        count += document.hits.length;
        if (performance.now() - began >= frame) {
          await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
          began = performance.now();
        }
      }
      this.total += count - (old?.count ?? 0);
      this.blocks.set(block.key, { parts, count });
    }
  }

  private interval: number | null = null;

  private async frameInterval(): Promise<number> {
    if (this.interval !== null) return this.interval;
    const frame = () => new Promise<number>((resolve) => requestAnimationFrame(resolve));
    const first = await frame();
    this.interval = (await frame()) - first;
    return this.interval;
  }

  search(query: string): void {
    if (query === this.query) return;
    this.query = query;
    this.total = 0;
    for (const block of this.blocks.values()) {
      block.count = 0;
      for (const part of block.parts) {
        part.hits = textMatches(part.text, query);
        block.count += part.hits.length;
      }
      this.total += block.count;
    }
  }

  at(position: number): FindMatch | null {
    if (position < 0) return null;
    for (const key of this.order) {
      const block = this.blocks.get(key);
      if (!block) continue;
      if (position >= block.count) {
        position -= block.count;
        continue;
      }
      for (const document of block.parts) {
        if (position < document.hits.length) return { document, ...document.hits[position]! };
        position -= document.hits.length;
      }
    }
    return null;
  }

  position(match: FindMatch): number {
    let position = 0;
    for (const key of this.order) {
      const block = this.blocks.get(key);
      if (!block) continue;
      if (key !== match.document.target.block) {
        position += block.count;
        continue;
      }
      for (const part of block.parts) {
        if (part.key === match.document.key) {
          const index = part.hits.findIndex((one) => one.start === match.start && one.end === match.end);
          return index < 0 ? -1 : position + index;
        }
        position += part.hits.length;
      }
      return -1;
    }
    return -1;
  }

  destroy(): void {
    this.lifetime.abort();
    this.blocks.clear();
    this.order = [];
    this.total = 0;
  }
}
