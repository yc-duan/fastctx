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
  /** Read from another machine (a change's lines): read where the page
   *  holds its block, not for what it does not hold. */
  remote?: boolean;
}

/** Where the page holds nothing, in the order of blocks: turns not held,
 *  the events `start..end`. */
export const GAP = "\u0000gap:";

/** Matches the shell counted in what the page does not hold: a part of a
 *  block, how many, where its block begins. */
export interface FarHits {
  block: string;
  part: string;
  count: number;
  at: number;
}

/** A match in what the page does not hold: the `ordinal`th in a part. */
export interface FarMatch {
  far: { block: string; part: string; ordinal: number; at: number };
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
 * parts are read and searched again during a stream. What the page does not
 * hold is counted by the shell (`find-far.ts`), its matches standing in
 * the gaps where those turns are. */
export class FindIndex {
  private blocks = new Map<string, { parts: Document[]; count: number }>();
  private order: readonly string[] = [];
  private query = "";
  private readonly lifetime = new AbortController();
  private local = 0;
  private far: FarHits[] = [];
  private farCount = 0;

  get total(): number {
    return this.local + this.farCount;
  }

  /** What the shell counted in what the page does not hold, in place of
   *  what it counted before. */
  setFar(hits: FarHits[]): void {
    this.far = [...hits].sort((a, b) => a.at - b.at);
    this.farCount = this.far.reduce((sum, hit) => sum + hit.count, 0);
  }

  /** The far hits within `start..end`. */
  private farIn(start: number, end: number): FarHits[] {
    let low = 0;
    let high = this.far.length;
    while (low < high) {
      const middle = (low + high) >> 1;
      if (this.far[middle]!.at < start) low = middle + 1;
      else high = middle;
    }
    const hits: FarHits[] = [];
    for (let index = low; index < this.far.length && this.far[index]!.at < end; index += 1) hits.push(this.far[index]!);
    return hits;
  }

  async update(changes: FindChanges): Promise<void> {
    this.order = changes.order;
    for (const key of changes.removed) {
      this.local -= this.blocks.get(key)?.count ?? 0;
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
      this.local += count - (old?.count ?? 0);
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
    this.local = 0;
    for (const block of this.blocks.values()) {
      block.count = 0;
      for (const part of block.parts) {
        part.hits = textMatches(part.text, query);
        block.count += part.hits.length;
      }
      this.local += block.count;
    }
  }

  at(position: number): FindMatch | FarMatch | null {
    if (position < 0) return null;
    for (const key of this.order) {
      if (key.startsWith(GAP)) {
        const [start, end] = key.slice(GAP.length).split(":").map(Number) as [number, number];
        for (const hit of this.farIn(start, end)) {
          if (position < hit.count) return { far: { block: hit.block, part: hit.part, ordinal: position, at: hit.at } };
          position -= hit.count;
        }
        continue;
      }
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

  /** The `ordinal`th match in a part the page holds, once it does. */
  match(block: string, part: string, ordinal: number): FindMatch | null {
    const document = this.blocks.get(block)?.parts.find((one) => one.key === part);
    const hit = document?.hits[ordinal];
    return document && hit ? { document, ...hit } : null;
  }

  position(match: FindMatch | FarMatch): number {
    let position = 0;
    if ("far" in match) {
      for (const key of this.order) {
        if (key.startsWith(GAP)) {
          const [start, end] = key.slice(GAP.length).split(":").map(Number) as [number, number];
          for (const hit of this.farIn(start, end)) {
            if (hit.block === match.far.block && hit.part === match.far.part) return match.far.ordinal < hit.count ? position + match.far.ordinal : -1;
            position += hit.count;
          }
          continue;
        }
        position += this.blocks.get(key)?.count ?? 0;
      }
      return -1;
    }
    for (const key of this.order) {
      if (key.startsWith(GAP)) {
        const [start, end] = key.slice(GAP.length).split(":").map(Number) as [number, number];
        for (const hit of this.farIn(start, end)) position += hit.count;
        continue;
      }
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
    this.local = 0;
    this.far = [];
    this.farCount = 0;
  }
}
